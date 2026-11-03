//! Cooked models: a model as the cooker writes it into a game's package, which a run from the
//! package reads in place of its glTF file and the files that names. It holds what [`gltf::read`]
//! makes of them, its primitives' triangles as the cooker optimized them, with their levels of
//! detail, and its images compressed to BC7 in KTX2, with their mip
//! levels, as [`read_ktx2`] reads them, so that a run neither decodes PNGs and JPEGs nor makes
//! levels for them, but takes the levels' blocks out of zstd, as the loader does.
//!
//! The format, every number little-endian, every count and length 32 bits, a string its length
//! then its UTF-8:
//!
//! ```text
//! ignored     a count, then each thing the model has that the engine ignores, as a string
//! parts       a count, then each part's translation (3 f32s), rotation (4) and scale (3), then
//!             its parent's index and its primitive's, each u32::MAX for none
//! primitives  a count, then each one's name, as a string; its vertices' count, then their
//!             positions (3 f32s each), their texture coordinates (2) and their colors (4); its
//!             indices' count and its indices; its levels' count and each one's start, count and
//!             error (an f32); its material's color (4 f32s), image (u32::MAX for none), whether
//!             it's nearest (a byte), its wrap (a byte: clamp, repeat, mirror), its alpha mode (a
//!             byte: opaque, mask, blend), the mask's cutoff (an f32), and whether it's
//!             double-sided (a byte)
//! images      a count, then each one's KTX2, as its length and its bytes
//! ```
//!
//! Its floats are the bits the reader made, so that a model's nodes, which reach the world hash,
//! are the same from a package as from the model's files.

use std::path::Path;

use gg_core::math::{Quat, Vec3};
use gg_core::{Error, Result};
use gg_renderer::{AlphaMode, Lod, Wrap};

use crate::Transform;
use crate::gltf::{self, Imported, Material, Part, Primitive};
use crate::image::read_ktx2;
use crate::package::{Files, Kind};

/// What stands for none, where the format holds an index.
const NONE: u32 = u32::MAX;

/// Model `imported`, as the cooker writes it into a game's package, its images given as `images`,
/// their KTX2 in its images' order, in place of those `imported` has.
///
/// # Panics
///
/// If `images` aren't as many as `imported`'s, or a count or a length is past 32 bits.
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
    }
    count(&mut bytes, imported.primitives.len());
    for primitive in &imported.primitives {
        string(&mut bytes, &primitive.name);
        count(&mut bytes, primitive.positions.len());
        floats(&mut bytes, primitive.positions.as_flattened());
        floats(&mut bytes, primitive.uvs.as_flattened());
        floats(&mut bytes, primitive.colors.as_flattened());
        count(&mut bytes, primitive.indices.len());
        for index in &primitive.indices {
            bytes.extend_from_slice(&index.to_le_bytes());
        }
        count(&mut bytes, primitive.lods.len());
        for lod in &primitive.lods {
            bytes.extend_from_slice(&lod.start.to_le_bytes());
            bytes.extend_from_slice(&lod.count.to_le_bytes());
            floats(&mut bytes, &[lod.error]);
        }
        let material = &primitive.material;
        floats(&mut bytes, &material.color);
        index(&mut bytes, material.image);
        let (alpha, cutoff) = match material.alpha {
            AlphaMode::Opaque => (0, 0.0),
            AlphaMode::Mask { cutoff } => (1, cutoff),
            AlphaMode::Blend => (2, 0.0),
        };
        let wrap = match material.wrap {
            Wrap::Clamp => 0,
            Wrap::Repeat => 1,
            Wrap::Mirror => 2,
        };
        bytes.extend_from_slice(&[u8::from(material.nearest), wrap, alpha]);
        floats(&mut bytes, &[cutoff]);
        bytes.push(u8::from(material.double_sided));
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
/// holds what [`gltf::read`] would give: each part after its parent, primitives of whole triangles
/// of their own vertices, levels within their indices, errors that don't shrink, and images that
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
        let part_count = self.count(48)?;
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
            parts.push(Part {
                transform,
                parent,
                primitive,
            });
        }
        let mut held = 0;
        let mut primitives = Vec::new();
        for i in 0..self.count(1)? {
            let primitive = self
                .primitive()
                .map_err(|problem| format!("its primitive {i} {problem}"))?;
            held += primitive.positions.len() * size_of::<[f32; 9]>()
                + primitive.indices.len() * size_of::<u32>();
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
        let mut samples = primitives
            .iter()
            .filter_map(|primitive| primitive.material.image);
        if let Some(i) = samples.find(|&i| i >= images.len()) {
            return Err(format!("a material samples image {i}, which it hasn't"));
        }
        Ok(Imported {
            parts,
            primitives,
            images,
            ignored,
        })
    }

    /// A primitive. The error says what's wrong with it.
    fn primitive(&mut self) -> Result<Primitive, String> {
        let name = self.string()?;
        let vertices = self.count(36)?;
        let positions = self.vectors(vertices)?;
        let uvs = self.vectors(vertices)?;
        let colors = self.vectors(vertices)?;
        let count = self.count(4)?;
        let indices: Vec<u32> = (0..count).map(|_| self.u32()).collect::<Result<_, _>>()?;
        if indices.is_empty() || !indices.len().is_multiple_of(3) {
            return Err(format!(
                "has {} indices, not whole triangles",
                indices.len()
            ));
        }
        if let Some(index) = indices.iter().find(|&&index| index as usize >= vertices) {
            return Err(format!("has index {index}, past its {vertices} vertices"));
        }
        let mut lods: Vec<Lod> = Vec::new();
        for i in 0..self.count(12)? {
            let (start, count) = (self.u32()?, self.u32()?);
            let [error] = self.floats()?;
            let end = start.checked_add(count).map(|end| end as usize);
            if count == 0 || !count.is_multiple_of(3) || end.is_none_or(|end| end > indices.len()) {
                return Err(format!(
                    "has level {i} of indices {start} on, {count} of them, not whole triangles \
                     among its {}",
                    indices.len()
                ));
            }
            let finer = lods.last().map_or(0.0, |finer| finer.error);
            if !error.is_finite() || error < finer {
                return Err(format!(
                    "has level {i} of error {error}, less than the level before's or not finite"
                ));
            }
            lods.push(Lod {
                start,
                count,
                error,
            });
        }
        let color = self.floats()?;
        let image = self.index()?;
        let [nearest, wrap, alpha]: [u8; 3] = self.take(3)?.try_into().expect("three bytes");
        let [cutoff] = self.floats()?;
        let [double_sided]: [u8; 1] = self.take(1)?.try_into().expect("a byte");
        let wrap = match wrap {
            0 => Wrap::Clamp,
            1 => Wrap::Repeat,
            2 => Wrap::Mirror,
            _ => return Err(format!("has a material that wraps by {wrap}")),
        };
        let alpha = match alpha {
            0 => AlphaMode::Opaque,
            1 => AlphaMode::Mask { cutoff },
            2 => AlphaMode::Blend,
            _ => return Err(format!("has a material of alpha mode {alpha}")),
        };
        let flag = |byte: u8, what: &str| match byte {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(format!("has a material whose {what} is {byte}")),
        };
        Ok(Primitive {
            name,
            positions,
            uvs,
            colors,
            indices,
            lods,
            material: Material {
                color,
                image,
                nearest: flag(nearest, "nearest")?,
                wrap,
                alpha,
                double_sided: flag(double_sided, "double-sided")?,
            },
        })
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

    /// `count` vectors of `N` floats each, the count checked against the bytes left.
    fn vectors<const N: usize>(&mut self, count: usize) -> Result<Vec<[f32; N]>, String> {
        if count.saturating_mul(N * 4) > self.bytes.len() {
            return Err(format!("it ends before its {count} vertices"));
        }
        (0..count).map(|_| self.floats()).collect()
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

    /// A model of two parts, the second under the first and drawing a primitive of two levels with
    /// a masked material that samples its image, and the KTX2 of that image, a block 4 texels
    /// across and down.
    fn model() -> (Imported, Vec<Vec<u8>>) {
        let image = ktx2(ktx2::Format::BC7_SRGB_BLOCK, [4, 4], &[vec![7; 16]], true);
        let primitive = Primitive {
            name: "square/0".to_owned(),
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]],
            uvs: vec![[0.0; 2], [1.0, 0.0], [0.0, 1.0], [1.0; 2]],
            colors: vec![[1.0; 4], [0.5; 4], [0.25; 4], [0.0, 0.0, 0.0, 1.0]],
            indices: vec![0, 1, 2, 2, 1, 3, 0, 1, 2],
            lods: vec![
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
            ],
            material: Material {
                color: [1.0, 0.5, 0.25, 1.0],
                image: Some(0),
                nearest: true,
                wrap: Wrap::Mirror,
                alpha: AlphaMode::Mask { cutoff: 0.25 },
                double_sided: true,
            },
        };
        let turned = Transform {
            translation: Vec3::new(1.0, 2.0, 3.0),
            rotation: Quat::from_rotation_y(0.1),
            scale: Vec3::new(1.0, -2.0, 0.5),
        };
        let parts = vec![
            Part {
                transform: turned,
                parent: None,
                primitive: None,
            },
            Part {
                transform: Transform::IDENTITY,
                parent: Some(0),
                primitive: Some(0),
            },
        ];
        let read = crate::image::read_ktx2(image.clone()).expect("a texture");
        let imported = Imported {
            parts,
            primitives: vec![primitive],
            images: vec![read],
            ignored: vec!["1 animation".to_owned()],
        };
        (imported, vec![image])
    }

    #[test]
    fn a_cooked_model_reads_back_as_it_was_written() {
        let (imported, images) = model();
        let bytes = encode(&imported, &images);
        let read = decode("m", Path::new("m.glb"), &bytes).expect("a cooked model");
        assert_eq!(read, imported);
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
            edited(&|imported| imported.parts[1].primitive = Some(1)),
        ));
        let primitive = |edit: fn(&mut Primitive)| {
            move |imported: &mut Imported| edit(&mut imported.primitives[0])
        };
        cases.push((
            "an index past its vertices",
            edited(&primitive(|primitive| primitive.indices[8] = 4)),
        ));
        cases.push((
            "indices that aren't whole triangles",
            edited(&primitive(|primitive| {
                primitive.indices.pop();
                primitive.lods.pop();
            })),
        ));
        cases.push((
            "a level past its indices",
            edited(&primitive(|primitive| primitive.lods[1].start = 7)),
        ));
        cases.push((
            "a level of no triangles",
            edited(&primitive(|primitive| primitive.lods[1].count = 0)),
        ));
        cases.push((
            "an error that shrinks",
            edited(&primitive(|primitive| primitive.lods[0].error = 1.0)),
        ));
        cases.push((
            "an error that isn't finite",
            edited(&primitive(|primitive| primitive.lods[1].error = f32::NAN)),
        ));
        cases.push((
            "a material sampling what it hasn't",
            edited(&primitive(|primitive| primitive.material.image = Some(1))),
        ));
        let mut image = images.clone();
        image[0][0] ^= 1;
        cases.push(("an image that isn't KTX2", encode(&imported, &image)));
        for (case, bytes) in cases {
            let error = decode("m", Path::new("m.glb"), &bytes).expect_err(case);
            assert_eq!(error.code(), "engine.bad_model", "{case}");
            assert!(error.to_string().contains("\"m\""), "{case}: {error}");
        }
    }
}
