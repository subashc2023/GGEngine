//! A model's data: accessors, over buffer views, over buffers from its GLB, files or data URIs.

use std::borrow::Cow;
use std::io;
use std::path::Path;

use gg_core::Result;

use super::{Reader, json};

const BYTE: u32 = 5120;
pub(super) const UNSIGNED_BYTE: u32 = 5121;
const SHORT: u32 = 5122;
const UNSIGNED_SHORT: u32 = 5123;
const UNSIGNED_INT: u32 = 5125;
pub(super) const FLOAT: u32 = 5126;

impl<'a, F: Fn(&Path, u64) -> io::Result<Vec<u8>>> Reader<'a, F> {
    /// Accessor `index`, which `what` reads as attribute `semantic`, as [`floats`](Self::floats)
    /// reads it, failing if it holds a number that isn't finite, which glTF doesn't allow.
    pub(super) fn attribute(
        &mut self,
        what: &str,
        semantic: &str,
        index: u32,
        components: &[usize],
        normalized: bool,
    ) -> Result<usize> {
        let model = self.model;
        let (count, finite) = self.floats(what, semantic, index, components, normalized)?;
        if !finite {
            return Err(model.bad(format!("accessor {index} holds a number that isn't finite")));
        }
        Ok(count)
    }

    /// Accessor `index`, which `what` reads as attribute `semantic`, once, all its elements'
    /// components in order into `floats`: how many components each element has, and whether
    /// they're all finite. glTF allows it if each element has a number of components in
    /// `components`, and they're floats, or, if `normalized`, normalized unsigned bytes or shorts.
    pub(super) fn floats(
        &mut self,
        what: &str,
        semantic: &str,
        index: u32,
        components: &[usize],
        normalized: bool,
    ) -> Result<(usize, bool)> {
        self.floats_of(what, semantic, index, components, normalized, false)
    }

    /// Accessor `index`, as [`floats`](Self::floats) reads it, of normalized signed bytes or
    /// shorts too, if `signed`, each 1 at its most and -1 at its least and the one past it, as
    /// glTF allows for an animation's rotations.
    pub(super) fn floats_of(
        &mut self,
        what: &str,
        semantic: &str,
        index: u32,
        components: &[usize],
        normalized: bool,
        signed: bool,
    ) -> Result<(usize, bool)> {
        let (model, gltf) = (self.model, self.gltf);
        let accessor = model.item(&gltf.accessors, "accessor", index)?;
        let component = match (accessor.component_type, accessor.normalized) {
            (FLOAT, false) => Some(Component::Float),
            (UNSIGNED_BYTE, true) if normalized => Some(Component::Byte),
            (UNSIGNED_SHORT, true) if normalized => Some(Component::Short),
            (BYTE, true) if normalized && signed => Some(Component::SignedByte),
            (SHORT, true) if normalized && signed => Some(Component::SignedShort),
            _ => None,
        };
        let count = match accessor.kind.as_str() {
            "SCALAR" => Some(1),
            "VEC2" => Some(2),
            "VEC3" => Some(3),
            "VEC4" => Some(4),
            "MAT4" => Some(16),
            _ => None,
        };
        let (Some(component), Some(count)) = (component, count.filter(|c| components.contains(c)))
        else {
            return Err(model.bad(format!(
                "{what} reads {semantic} from accessor {index}, {}, which glTF doesn't allow for it",
                accessor.describe()
            )));
        };
        if !self.floats.contains_key(&index) {
            let size = component.size();
            // Checked first, so that a count past the bytes there are fails as that, then counted.
            self.accessor_bytes(index, accessor, count * size)?;
            let len = accessor.count as usize * count;
            self.memory.hold(len * size_of::<f32>())?;
            let (bytes, stride) = self.accessor_bytes(index, accessor, count * size)?;
            let elements = bytes.chunks(stride).map(|element| &element[..count * size]);
            let floats: Vec<f32> = match component {
                Component::Float => (elements.flat_map(|element| element.as_chunks::<4>().0))
                    .map(|&float| f32::from_le_bytes(float))
                    .collect(),
                Component::Byte => (elements.flatten())
                    .map(|&byte| f32::from(byte) / 255.0)
                    .collect(),
                Component::Short => (elements.flat_map(|element| element.as_chunks::<2>().0))
                    .map(|&short| f32::from(u16::from_le_bytes(short)) / 65535.0)
                    .collect(),
                Component::SignedByte => (elements.flatten())
                    .map(|&byte| (f32::from(byte as i8) / 127.0).max(-1.0))
                    .collect(),
                Component::SignedShort => (elements.flat_map(|element| element.as_chunks::<2>().0))
                    .map(|&short| (f32::from(i16::from_le_bytes(short)) / 32767.0).max(-1.0))
                    .collect(),
            };
            let finite = floats.iter().all(|float| float.is_finite());
            self.floats.insert(index, (floats, finite));
        }
        Ok((count, self.floats[&index].1))
    }

    /// Accessor `index`'s elements of `N` components, which [`floats`](Self::floats) read, copied
    /// once the copy is counted.
    pub(super) fn copied<const N: usize>(&mut self, index: u32) -> Result<Vec<[f32; N]>> {
        let floats = self.floats[&index].0.as_chunks::<N>().0;
        self.memory.copy(floats)
    }

    /// Accessor `index`, which `what` reads as JOINTS_0: each vertex's four joints, of unsigned
    /// bytes or shorts, as glTF allows.
    pub(super) fn joints(&mut self, what: &str, index: u32) -> Result<Vec<[u16; 4]>> {
        let (model, gltf) = (self.model, self.gltf);
        let accessor = model.item(&gltf.accessors, "accessor", index)?;
        let size = match (
            accessor.kind.as_str(),
            accessor.component_type,
            accessor.normalized,
        ) {
            ("VEC4", UNSIGNED_BYTE, false) => 1,
            ("VEC4", UNSIGNED_SHORT, false) => 2,
            _ => {
                return Err(model.bad(format!(
                    "{what} reads JOINTS_0 from accessor {index}, {}, which glTF doesn't allow \
                     for it",
                    accessor.describe()
                )));
            }
        };
        // Checked first, so that a count past the bytes there are fails as that, then counted.
        self.accessor_bytes(index, accessor, 4 * size)?;
        self.memory
            .hold(accessor.count as usize * size_of::<[u16; 4]>())?;
        let (bytes, stride) = self.accessor_bytes(index, accessor, 4 * size)?;
        Ok((bytes.chunks(stride))
            .map(|element| {
                std::array::from_fn(|i| match size {
                    1 => u16::from(element[i]),
                    _ => u16::from_le_bytes([element[2 * i], element[2 * i + 1]]),
                })
            })
            .collect())
    }

    /// Accessor `index`, which `what` reads as its indices, once, into `indices`, checked to be
    /// whole triangles, each of `vertices` vertices.
    pub(super) fn indices(&mut self, what: &str, index: u32, vertices: u32) -> Result<()> {
        let (model, gltf) = (self.model, self.gltf);
        let accessor = model.item(&gltf.accessors, "accessor", index)?;
        let size = match (
            accessor.kind.as_str(),
            accessor.component_type,
            accessor.normalized,
        ) {
            ("SCALAR", UNSIGNED_BYTE, false) => 1,
            ("SCALAR", UNSIGNED_SHORT, false) => 2,
            ("SCALAR", UNSIGNED_INT, false) => 4,
            _ => {
                return Err(model.bad(format!(
                    "{what} reads indices from accessor {index}, {}, which glTF doesn't allow \
                     for them",
                    accessor.describe()
                )));
            }
        };
        if !accessor.count.is_multiple_of(3) {
            return Err(model.bad(format!(
                "{what} has {} indices, which isn't whole triangles",
                accessor.count
            )));
        }
        if !self.indices.contains_key(&index) {
            // Checked first, so that a count past the bytes there are fails as that, then counted.
            self.accessor_bytes(index, accessor, size)?;
            self.memory
                .hold(accessor.count as usize * size_of::<u32>())?;
            let (bytes, stride) = self.accessor_bytes(index, accessor, size)?;
            let indices = (bytes.chunks(stride))
                .map(|element| match element[..size] {
                    [byte] => u32::from(byte),
                    [a, b] => u32::from(u16::from_le_bytes([a, b])),
                    [a, b, c, d] => u32::from_le_bytes([a, b, c, d]),
                    _ => unreachable!("indices are 1, 2 or 4 bytes"),
                })
                .collect();
            self.indices.insert(index, indices);
        }
        let indices = &self.indices[&index];
        if let Some((at, &past)) = indices.iter().enumerate().find(|&(_, &i)| i >= vertices) {
            return Err(model.bad(format!(
                "{what} has index {past} at {at}, past its {vertices} vertices"
            )));
        }
        Ok(())
    }

    /// The bytes of accessor `index`'s elements, of `size` bytes each, from the first's start to the
    /// last's end, and the stride from each to the next, once they're checked to be within its
    /// buffer view, and that within its buffer.
    fn accessor_bytes(
        &mut self,
        index: u32,
        accessor: &json::Accessor,
        size: usize,
    ) -> Result<(&[u8], usize)> {
        let model = self.model;
        if accessor.sparse.is_some() {
            return Err(model.unsupported(format!(
                "accessor {index} is sparse, which the engine doesn't read"
            )));
        }
        let view_index = accessor.buffer_view.ok_or_else(|| {
            model.unsupported(format!(
                "accessor {index} has no buffer view, which the engine doesn't read"
            ))
        })?;
        if accessor.count == 0 {
            return Err(model.bad(format!("accessor {index} has no elements")));
        }
        let (view, bytes) = self.view(view_index)?;
        let size = size as u64;
        let stride = view.byte_stride.unwrap_or(size);
        if stride < size {
            return Err(model.bad(format!(
                "accessor {index}'s elements are {size} bytes, more than buffer view \
                 {view_index}'s stride of {stride}"
            )));
        }
        // Checked before anything is allocated for the elements, so that a count far past the
        // bytes there are fails rather than allocating for it.
        let end = (accessor.count - 1)
            .checked_mul(stride)
            .and_then(|start| start.checked_add(size))
            .and_then(|length| length.checked_add(accessor.byte_offset))
            .filter(|&end| end <= bytes.len() as u64)
            .ok_or_else(|| {
                model.bad(format!(
                    "accessor {index} reads past buffer view {view_index}"
                ))
            })?;
        Ok((
            &bytes[accessor.byte_offset as usize..end as usize],
            stride as usize,
        ))
    }

    /// Buffer view `index`, and its bytes, once they're checked to be within its buffer.
    pub(super) fn view(&mut self, index: u32) -> Result<(&'a json::BufferView, &[u8])> {
        let (model, gltf) = (self.model, self.gltf);
        let view = model.item(&gltf.buffer_views, "buffer view", index)?;
        let buffer = self.buffer(view.buffer)?;
        let range = (view.byte_offset.checked_add(view.byte_length))
            .filter(|&end| end <= buffer.len() as u64)
            .map(|end| view.byte_offset as usize..end as usize)
            .ok_or_else(|| {
                model.bad(format!(
                    "buffer view {index} reads past buffer {}",
                    view.buffer
                ))
            })?;
        Ok((view, &buffer[range]))
    }

    /// Buffer `index`'s bytes, read once: exactly its byteLength.
    fn buffer(&mut self, index: u32) -> Result<&[u8]> {
        if !self.buffers.contains_key(&index) {
            let (model, gltf) = (self.model, self.gltf);
            let buffer = model.item(&gltf.buffers, "buffer", index)?;
            let bytes = match (&buffer.uri, self.bin) {
                // No further into a file than its byteLength, nor than a byte past the memory
                // reading may still take.
                (Some(uri), _) => {
                    let room =
                        (self.memory.most.saturating_sub(self.memory.held)).saturating_add(1);
                    let most = buffer.byte_length.min(room);
                    Cow::Owned(self.uri(&format!("buffer {index}"), uri, most)?)
                }
                (None, Some(bin)) if index == 0 => Cow::Borrowed(bin),
                (None, _) => {
                    return Err(model.bad(format!(
                        "buffer {index} has no uri, and isn't a GLB's BIN chunk"
                    )));
                }
            };
            // Read for each buffer, as buffers that share a file each read it.
            if let Cow::Owned(bytes) = &bytes {
                self.memory.hold(bytes.len())?;
            }
            let length = (usize::try_from(buffer.byte_length).ok())
                .filter(|&length| length <= bytes.len())
                .ok_or_else(|| {
                    model.bad(format!(
                        "buffer {index} is {} bytes, short of its byteLength of {}",
                        bytes.len(),
                        buffer.byte_length
                    ))
                })?;
            let bytes = match bytes {
                Cow::Borrowed(bytes) => Cow::Borrowed(&bytes[..length]),
                Cow::Owned(mut bytes) => {
                    bytes.truncate(length);
                    Cow::Owned(bytes)
                }
            };
            self.buffers.insert(index, bytes);
        }
        Ok(&self.buffers[&index])
    }

    /// The bytes that `uri`, `what`'s, names: a data URI's, or those of a file in the model's
    /// folder, as far as `most` bytes.
    pub(super) fn uri(&self, what: &str, uri: &str, most: u64) -> Result<Vec<u8>> {
        let model = self.model;
        if uri
            .get(..5)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("data:"))
        {
            let (header, data) = uri.split_once(',').unwrap_or((uri, ""));
            if !header.ends_with(";base64") {
                return Err(model.bad(format!("{what}'s data URI isn't base64")));
            }
            return base64(data)
                .ok_or_else(|| model.bad(format!("{what}'s data URI isn't valid base64")));
        }
        let path = percent_decode(uri).ok_or_else(|| {
            model.bad(format!(
                "{what}'s URI {uri:?} has a % that isn't followed by two hex digits, or isn't \
                 UTF-8"
            ))
        })?;
        // Relative paths alone, as glTF has them: no scheme, drive or root, and no backslash,
        // which Windows would take for a separator.
        if path.starts_with('/') || path.contains(['\\', ':']) {
            return Err(model.bad(format!("{what}'s URI {uri:?} isn't a relative path")));
        }
        // And none up a folder, so that a model reads its own folder's files alone.
        if path.split('/').any(|segment| segment == "..") {
            return Err(model.bad(format!(
                "{what}'s URI {uri:?} has a `..`, where a model reads files in its folder alone"
            )));
        }
        let file = model.path.parent().unwrap_or(Path::new("")).join(path);
        (self.read_file)(&file, most).map_err(|error| {
            model
                .bad(format!("{what}'s file {} can't be read", file.display()))
                .with_source(error)
        })
    }
}

/// The type of an attribute's components that glTF allows: floats, unsigned bytes or shorts
/// normalized to 0 to 1, or, for an animation's rotations, signed ones normalized to -1 to 1.
#[derive(Clone, Copy)]
enum Component {
    Float,
    Byte,
    Short,
    SignedByte,
    SignedShort,
}

impl Component {
    fn size(self) -> usize {
        match self {
            Self::Float => 4,
            Self::Byte | Self::SignedByte => 1,
            Self::Short | Self::SignedShort => 2,
        }
    }
}

/// Decodes base64 as RFC 4648 has it, with its padding, as data URIs hold it: `None` for a
/// character outside its alphabet, or padding that's missing or out of place.
pub(crate) fn base64(text: &str) -> Option<Vec<u8>> {
    let text = text.as_bytes();
    if !text.len().is_multiple_of(4) {
        return None;
    }
    let quads = text.as_chunks::<4>().0;
    let mut bytes = Vec::with_capacity(quads.len() * 3);
    for (i, quad) in quads.iter().enumerate() {
        let padding = quad.iter().rev().take_while(|&&c| c == b'=').count();
        if padding > 2 || (padding > 0 && i + 1 < quads.len()) {
            return None;
        }
        let mut bits = 0;
        for &c in &quad[..4 - padding] {
            let sextet = match c {
                b'A'..=b'Z' => c - b'A',
                b'a'..=b'z' => c - b'a' + 26,
                b'0'..=b'9' => c - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                _ => return None,
            };
            bits = bits << 6 | u32::from(sextet);
        }
        let [_, decoded @ ..] = (bits << (6 * padding)).to_be_bytes();
        bytes.extend_from_slice(&decoded[..3 - padding]);
    }
    Some(bytes)
}

/// Decodes a URI's percent escapes: `None` for a `%` not followed by two hex digits, or for what
/// isn't UTF-8.
pub(super) fn percent_decode(uri: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(uri.len());
    let mut rest = uri.as_bytes();
    while let Some((&byte, after)) = rest.split_first() {
        if byte == b'%' {
            let hex = |at: usize| char::from(*after.get(at)?).to_digit(16);
            bytes.push((hex(0)? * 16 + hex(1)?) as u8);
            rest = &after[2..];
        } else {
            bytes.push(byte);
            rest = after;
        }
    }
    String::from_utf8(bytes).ok()
}
