//! The SPIR-V slangc writes: checking a shader against the contract that
//! [`Shaders::add`](super::Shaders::add) describes, which Vulkan checks none of, and decorating its
//! sampled images `NonUniform` as Vulkan requires. Both read the module's instructions as the
//! SPIR-V specification lays them out, each a word of its length and opcode, then its operands.

use std::collections::HashMap;
#[cfg(test)]
use std::collections::HashSet;

mod check;
mod patch;

pub(super) use check::breaches;
pub(super) use patch::{decorate_non_uniform, return_no_motion};

/// SPIR-V's magic number, which opens a module.
const MAGIC: u32 = 0x0723_0203;

/// The most a module's bound, one past its every ID, may be: SPIR-V's universal limit, which
/// spirv-val holds modules to, and which keeps the IDs [`return_no_motion`] adds within a word.
const MOST_BOUND: u32 = 4_194_303;

// The opcodes read.
const OP_NAME: u32 = 5;
const OP_MEMBER_NAME: u32 = 6;
const OP_ENTRY_POINT: u32 = 15;
const OP_CAPABILITY: u32 = 17;
const OP_TYPE_BOOL: u32 = 20;
const OP_TYPE_INT: u32 = 21;
const OP_TYPE_FLOAT: u32 = 22;
const OP_TYPE_VECTOR: u32 = 23;
const OP_TYPE_MATRIX: u32 = 24;
const OP_TYPE_IMAGE: u32 = 25;
const OP_TYPE_SAMPLER: u32 = 26;
const OP_TYPE_SAMPLED_IMAGE: u32 = 27;
const OP_TYPE_ARRAY: u32 = 28;
const OP_TYPE_RUNTIME_ARRAY: u32 = 29;
const OP_TYPE_STRUCT: u32 = 30;
const OP_TYPE_POINTER: u32 = 32;
const OP_CONSTANT: u32 = 43;
const OP_CONSTANT_NULL: u32 = 46;
const OP_FUNCTION: u32 = 54;
const OP_FUNCTION_END: u32 = 56;
const OP_VARIABLE: u32 = 59;
const OP_STORE: u32 = 62;
const OP_COPY_MEMORY: u32 = 63;
const OP_COPY_MEMORY_SIZED: u32 = 64;
const OP_DECORATE: u32 = 71;
const OP_MEMBER_DECORATE: u32 = 72;
const OP_SAMPLED_IMAGE: u32 = 86;
const OP_RETURN: u32 = 253;
const OP_ATOMIC_STORE: u32 = 228;
const OP_ATOMIC_FLAG_CLEAR: u32 = 319;

/// The instructions that make a value of a type, a pointer among them, which the check follows to
/// see what a store writes through: `OpUndef`, `OpFunctionParameter`, `OpFunctionCall`, `OpLoad`,
/// the access chains, `OpCompositeExtract`, `OpCopyObject`, `OpConvertUToPtr`, `OpBitcast`,
/// `OpSelect` and `OpPhi`.
const MAKES_VALUES: [u32; 14] = [1, 55, 57, 61, 65, 66, 67, 70, 81, 83, 120, 124, 169, 245];

/// Whether `opcode` is an atomic operation's other than `OpAtomicStore` and `OpAtomicFlagClear`,
/// whose pointer comes after their result's type and ID.
fn is_atomic(opcode: u32) -> bool {
    matches!(opcode, 227 | 229..=242 | 318 | 5614 | 5615 | 6035)
}

// Decorations.
const ARRAY_STRIDE: u32 = 6;
const MATRIX_STRIDE: u32 = 7;
const BUILT_IN: u32 = 11;
const LOCATION: u32 = 30;
const BINDING: u32 = 33;
const DESCRIPTOR_SET: u32 = 34;
const OFFSET: u32 = 35;
const NON_UNIFORM: u32 = 5300;
const BUFFER_BLOCK: u32 = 3;

// Storage classes.
const UNIFORM_CONSTANT: u32 = 0;
const INPUT: u32 = 1;
const UNIFORM: u32 = 2;
const OUTPUT: u32 = 3;
const PUSH_CONSTANT: u32 = 9;
const STORAGE_BUFFER: u32 = 12;
const PHYSICAL_STORAGE_BUFFER: u32 = 5349;

// Execution models.
const VERTEX: u32 = 0;
const FRAGMENT: u32 = 4;

// Built-ins.
const SAMPLE_MASK: u32 = 20;
const FRAG_DEPTH: u32 = 22;
const VERTEX_INDEX: u32 = 42;
const INSTANCE_INDEX: u32 = 43;
const BASE_VERTEX: u32 = 4424;
const BASE_INSTANCE: u32 = 4425;
const DRAW_INDEX: u32 = 4426;

/// An image's dimensionality, for 2D images.
const DIM_2D: u32 = 1;
const DIM_CUBE: u32 = 3;

/// A type, as far as the check reads types.
enum Type {
    Bool,
    Int {
        width: u32,
        signed: bool,
    },
    Float {
        width: u32,
    },
    Vector {
        component: u32,
        count: u32,
    },
    Matrix {
        column: u32,
        count: u32,
    },
    Image(Image),
    Sampler,
    SampledImage,
    Array {
        element: u32,
        /// The ID of the constant that's its length.
        length: u32,
    },
    RuntimeArray {
        element: u32,
    },
    Struct {
        members: Vec<u32>,
    },
    Pointer {
        storage: u32,
        pointee: u32,
    },
}

struct Image {
    /// The type of what's sampled from it.
    sampled: u32,
    dim: u32,
    arrayed: bool,
    multisampled: bool,
    /// 1 for an image that's sampled, 2 for one that's read and written.
    usage: u32,
}

impl Image {
    /// Whether it's a 2D image or a cube that's sampled, as the bindless set's textures are.
    fn is_texture(&self) -> bool {
        matches!(self.dim, DIM_2D | DIM_CUBE)
            && (self.arrayed, self.multisampled, self.usage) == (false, false, 1)
    }
}

struct EntryPoint {
    model: u32,
    name: String,
    /// The global variables it uses: its inputs and outputs among them.
    interface: Vec<u32>,
}

/// A variable: its type, a pointer, and its storage class.
struct Variable {
    id: u32,
    ty: u32,
    storage: u32,
}

/// What a module says, as far as the check reads it.
#[derive(Default)]
struct Module {
    capabilities: Vec<u32>,
    entry_points: Vec<EntryPoint>,
    names: HashMap<u32, String>,
    member_names: HashMap<(u32, u32), String>,
    /// Each ID's decorations, with their first operand, if they have one.
    decorations: HashMap<(u32, u32), u32>,
    /// Each struct member's, by struct and member.
    member_decorations: HashMap<(u32, u32, u32), u32>,
    types: HashMap<u32, Type>,
    /// The first word of each constant's value.
    constants: HashMap<u32, u32>,
    variables: Vec<Variable>,
    /// The type of each value that can be a pointer.
    value_types: HashMap<u32, u32>,
    /// What stores, copies and atomic operations reach through.
    written: Vec<u32>,
}

impl Module {
    /// Reads `spirv`: `None` if it isn't SPIR-V that can be read.
    fn read(spirv: &[u8]) -> Option<Self> {
        let words = words(spirv)?;
        let mut module = Self::default();
        for instruction in instructions(&words) {
            let (_, opcode, operands) = instruction?;
            module.take(opcode, operands)?;
        }
        Some(module)
    }

    /// Notes what an instruction says. `None` if it can't be read.
    fn take(&mut self, opcode: u32, operands: &[u32]) -> Option<()> {
        match (opcode, operands) {
            (OP_CAPABILITY, &[capability]) => self.capabilities.push(capability),
            (OP_ENTRY_POINT, &[model, _, ref rest @ ..]) => {
                let (name, length) = string(rest)?;
                let interface = rest[length..].to_vec();
                self.entry_points.push(EntryPoint {
                    model,
                    name,
                    interface,
                });
            }
            (OP_NAME, &[id, ref name @ ..]) => {
                self.names.insert(id, string(name)?.0);
            }
            (OP_MEMBER_NAME, &[id, member, ref name @ ..]) => {
                self.member_names.insert((id, member), string(name)?.0);
            }
            (OP_DECORATE, &[id, decoration, ref value @ ..]) => {
                let value = value.first().copied().unwrap_or_default();
                self.decorations.insert((id, decoration), value);
            }
            (OP_MEMBER_DECORATE, &[id, member, decoration, ref value @ ..]) => {
                let value = value.first().copied().unwrap_or_default();
                self.member_decorations
                    .insert((id, member, decoration), value);
            }
            (OP_TYPE_BOOL, &[id]) => self.add_type(id, Type::Bool),
            (OP_TYPE_INT, &[id, width, signedness]) => {
                let signed = signedness != 0;
                self.add_type(id, Type::Int { width, signed });
            }
            (OP_TYPE_FLOAT, &[id, width, ..]) => self.add_type(id, Type::Float { width }),
            (OP_TYPE_VECTOR, &[id, component, count]) => {
                self.add_type(id, Type::Vector { component, count });
            }
            (OP_TYPE_MATRIX, &[id, column, count]) => {
                self.add_type(id, Type::Matrix { column, count });
            }
            (OP_TYPE_IMAGE, &[id, sampled, dim, _, arrayed, multisampled, usage, ..]) => {
                let image = Image {
                    sampled,
                    dim,
                    arrayed: arrayed != 0,
                    multisampled: multisampled != 0,
                    usage,
                };
                self.add_type(id, Type::Image(image));
            }
            (OP_TYPE_SAMPLER, &[id]) => self.add_type(id, Type::Sampler),
            (OP_TYPE_SAMPLED_IMAGE, &[id, _]) => self.add_type(id, Type::SampledImage),
            (OP_TYPE_ARRAY, &[id, element, length]) => {
                self.add_type(id, Type::Array { element, length });
            }
            (OP_TYPE_RUNTIME_ARRAY, &[id, element]) => {
                self.add_type(id, Type::RuntimeArray { element });
            }
            (OP_TYPE_STRUCT, &[id, ref members @ ..]) => {
                let members = members.to_vec();
                self.add_type(id, Type::Struct { members });
            }
            (OP_TYPE_POINTER, &[id, storage, pointee]) => {
                self.add_type(id, Type::Pointer { storage, pointee });
            }
            (OP_CONSTANT, &[_, id, value, ..]) => {
                self.constants.insert(id, value);
            }
            (OP_VARIABLE, &[ty, id, storage, ..]) => {
                self.value_types.insert(id, ty);
                self.variables.push(Variable { id, ty, storage });
            }
            (
                OP_STORE | OP_COPY_MEMORY | OP_COPY_MEMORY_SIZED | OP_ATOMIC_STORE
                | OP_ATOMIC_FLAG_CLEAR,
                &[pointer, ..],
            ) => self.written.push(pointer),
            (opcode, &[_, _, pointer, ..]) if is_atomic(opcode) => self.written.push(pointer),
            (opcode, &[ty, id, ..]) if MAKES_VALUES.contains(&opcode) => {
                self.value_types.insert(id, ty);
            }
            _ => {}
        }
        Some(())
    }

    fn add_type(&mut self, id: u32, ty: Type) {
        self.types.insert(id, ty);
    }

    fn decoration(&self, id: u32, decoration: u32) -> Option<u32> {
        self.decorations.get(&(id, decoration)).copied()
    }

    fn member_decoration(&self, id: u32, member: u32, decoration: u32) -> Option<u32> {
        (self.member_decorations)
            .get(&(id, member, decoration))
            .copied()
    }

    /// What `id` is called in the shader's source: a struct by the name slangc gave it, without
    /// the layout slangc adds to it.
    fn name(&self, id: u32) -> String {
        let Some(name) = self.names.get(&id) else {
            return format!("%{id}");
        };
        let name = name.strip_suffix("_logical").unwrap_or(name);
        let name = (name.strip_suffix("_natural"))
            .or_else(|| name.strip_suffix("_std430"))
            .unwrap_or(name);
        name.to_owned()
    }

    /// What an entry point's input or output is called in the shader's source: a parameter, or a
    /// field of the struct it returns, as slangc names its variables, such as
    /// `entryPointParam_fragment_main.color`; `None` for what an entry point returns whole.
    fn interface_name(&self, id: u32) -> Option<String> {
        let name = self.names.get(&id)?;
        match name.rsplit_once('.') {
            Some((_, field)) => Some(field.to_owned()),
            None if name.starts_with("entryPointParam_") => None,
            None => Some(name.clone()),
        }
    }

    fn member_name(&self, id: u32, member: u32) -> String {
        (self.member_names.get(&(id, member)))
            .cloned()
            .unwrap_or_else(|| format!("member {member}"))
    }

    /// Whether `ty` is a 32-bit float.
    fn is_float(&self, ty: u32) -> bool {
        matches!(self.types.get(&ty), Some(Type::Float { width: 32 }))
    }

    /// Whether `ty` is a vector of two 32-bit integers, and if so whether they're signed: a uint2
    /// is `Some(false)`, and an int2 `Some(true)`.
    fn signed_int2(&self, ty: u32) -> Option<bool> {
        let Some(&Type::Vector {
            component,
            count: 2,
        }) = self.types.get(&ty)
        else {
            return None;
        };
        match self.types.get(&component) {
            Some(&Type::Int { width: 32, signed }) => Some(signed),
            _ => None,
        }
    }

    /// What `pointer` points at, if it's a pointer of storage class `storage`.
    fn pointee(&self, pointer: u32, storage: u32) -> Option<u32> {
        match self.types.get(&pointer) {
            Some(&Type::Pointer {
                storage: found,
                pointee,
            }) if found == storage => Some(pointee),
            _ => None,
        }
    }
}

/// `spirv` as words, if it's a whole number of them that opens as a SPIR-V module does, with a
/// bound within [`MOST_BOUND`].
fn words(spirv: &[u8]) -> Option<Vec<u32>> {
    let (words, []) = spirv.as_chunks::<4>() else {
        return None;
    };
    let words: Vec<u32> = words.iter().map(|&word| u32::from_le_bytes(word)).collect();
    (words.len() >= 5 && words[0] == MAGIC && words[3] <= MOST_BOUND).then_some(words)
}

/// The instructions of `words`, a module's, after its header, each a word of its length and its
/// opcode, then its operands: each one's position, opcode and operands, then `None` and no more
/// where one can't be read, as one of no words, or one that runs past the end.
fn instructions(words: &[u32]) -> impl Iterator<Item = Option<(usize, u32, &[u32])>> {
    let mut next = Some(5);
    std::iter::from_fn(move || {
        let at = next.filter(|&at| at < words.len())?;
        let length = (words[at] >> 16) as usize;
        // None for one of no words too, whose operands would end before they start.
        let operands = words.get(at + 1..at + length);
        next = operands.map(|_| at + length);
        Some(operands.map(|operands| (at, words[at] & 0xffff, operands)))
    })
}

/// The string that `words` open with, and how many words it takes: UTF-8, ending in a zero byte,
/// padded to a whole word.
fn string(words: &[u32]) -> Option<(String, usize)> {
    let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    let end = bytes.iter().position(|&byte| byte == 0)?;
    let text = String::from_utf8_lossy(&bytes[..end]).into_owned();
    Some((text, end / 4 + 1))
}

/// The locations of what `spirv`'s fragment shaders return, in order.
#[cfg(test)]
pub(super) fn fragment_locations(spirv: &[u8]) -> Vec<u32> {
    let module = Module::read(spirv).expect("SPIR-V");
    let outputs: HashSet<u32> = (module.variables.iter())
        .filter(|variable| variable.storage == OUTPUT)
        .map(|variable| variable.id)
        .collect();
    let fragments = (module.entry_points.iter()).filter(|entry| entry.model == FRAGMENT);
    let mut locations: Vec<u32> = fragments
        .flat_map(|entry| entry.interface.iter())
        .filter(|id| outputs.contains(id))
        .filter_map(|&id| module.decoration(id, LOCATION))
        .collect();
    locations.sort_unstable();
    locations
}

/// The IDs of the sampled images `spirv` makes, each with whether it's decorated NonUniform, and
/// whether the image or the sampler it's made of is.
#[cfg(test)]
pub(super) fn sampled_images(spirv: &[u8]) -> Vec<(u32, bool, bool)> {
    let words = words(spirv).expect("SPIR-V");
    let (mut non_uniform, mut sampled) = (HashSet::new(), Vec::new());
    for instruction in instructions(&words) {
        match instruction.expect("an instruction that can be read") {
            (_, OP_DECORATE, &[id, NON_UNIFORM]) => {
                non_uniform.insert(id);
            }
            (_, OP_SAMPLED_IMAGE, &[_, id, image, sampler]) => sampled.push([id, image, sampler]),
            _ => {}
        }
    }
    let decorated = |[id, image, sampler]: [u32; 3]| {
        let of = non_uniform.contains(&image) || non_uniform.contains(&sampler);
        (id, non_uniform.contains(&id), of)
    };
    sampled.into_iter().map(decorated).collect()
}

/// How many of the pointer types `spirv` declares point into buffer memory, by device address, at
/// a pointer: what a shader makes as it reaches into a buffer for a pointer the buffer holds,
/// rather than loading what holds it whole.
#[cfg(test)]
pub(super) fn buffer_pointers_to_pointers(spirv: &[u8]) -> usize {
    let words = words(spirv).expect("SPIR-V");
    let (mut pointers, mut into_buffers) = (HashSet::new(), Vec::new());
    for instruction in instructions(&words) {
        if let (_, OP_TYPE_POINTER, &[id, storage, pointee]) =
            instruction.expect("an instruction that can be read")
        {
            pointers.insert(id);
            if storage == PHYSICAL_STORAGE_BUFFER {
                into_buffers.push(pointee);
            }
        }
    }
    (into_buffers.iter())
        .filter(|pointee| pointers.contains(pointee))
        .count()
}

/// How many of the struct types `spirv` reaches in buffer memory, by device address, hold a
/// pointer, as a member or in a struct or array they hold.
#[cfg(test)]
pub(super) fn buffer_structs_holding_pointers(spirv: &[u8]) -> usize {
    let words = words(spirv).expect("SPIR-V");
    let (mut pointers, mut into_buffers) = (HashSet::new(), Vec::new());
    // The types each struct and array holds.
    let mut holds = HashMap::new();
    for instruction in instructions(&words) {
        match instruction.expect("an instruction that can be read") {
            (_, OP_TYPE_POINTER, &[id, storage, pointee]) => {
                pointers.insert(id);
                if storage == PHYSICAL_STORAGE_BUFFER {
                    into_buffers.push(pointee);
                }
            }
            (_, OP_TYPE_STRUCT, &[id, ref members @ ..]) => {
                holds.insert(id, members.to_vec());
            }
            (_, OP_TYPE_ARRAY | OP_TYPE_RUNTIME_ARRAY, &[id, element, ..]) => {
                holds.insert(id, vec![element]);
            }
            _ => {}
        }
    }
    fn holds_pointer(ty: u32, holds: &HashMap<u32, Vec<u32>>, pointers: &HashSet<u32>) -> bool {
        (holds.get(&ty).into_iter().flatten())
            .any(|&held| pointers.contains(&held) || holds_pointer(held, holds, pointers))
    }
    (into_buffers.iter())
        .filter(|&&pointee| holds_pointer(pointee, &holds, &pointers))
        .count()
}

#[cfg(test)]
mod tests {
    use std::mem::offset_of;

    use super::*;
    use crate::mesh::Push;
    use crate::shader::Types;

    #[test]
    fn sampled_images_of_non_uniform_images_or_samplers_are_non_uniform() {
        let module = |instructions: &[[u32; 3]], samples: &[[u32; 5]]| {
            let header = [MAGIC, 0x0001_0500, 0, 64, 0];
            let body = instructions
                .as_flattened()
                .iter()
                .chain(samples.as_flattened());
            let words = header.iter().chain(body);
            words
                .flat_map(|word| word.to_le_bytes())
                .collect::<Vec<_>>()
        };
        let decorate = |id| [(3 << 16) | OP_DECORATE, id, NON_UNIFORM];
        let sample = |id, image, sampler| [(5 << 16) | OP_SAMPLED_IMAGE, 1, id, image, sampler];
        // Image 10 and sampler 21 are non-uniform, image 11 and sampler 20 aren't; sampled image
        // 33 is decorated already.
        let samples = [
            sample(30, 10, 20),
            sample(31, 11, 21),
            sample(32, 11, 20),
            sample(33, 10, 21),
        ];
        let spirv = module(&[decorate(10), decorate(21), decorate(33)], &samples);
        let decorated = decorate_non_uniform(spirv.clone());
        let sampled = [
            (30, true, true),
            (31, true, true),
            (32, false, false),
            (33, true, true),
        ];
        assert_eq!(sampled_images(&decorated), sampled);
        // After the first NonUniform decoration, among the others.
        let expected = [
            decorate(10),
            decorate(30),
            decorate(31),
            decorate(21),
            decorate(33),
        ];
        assert_eq!(decorated, module(&expected, &samples));
        // What it can't read stays as it is: an instruction cut short, an instruction of no words,
        // bytes that aren't whole words, and what isn't SPIR-V.
        let mut empty = spirv.clone();
        empty.splice(20..20, [0; 4]);
        let unreadable = [
            spirv[..spirv.len() - 4].to_vec(),
            empty,
            spirv[..spirv.len() - 1].to_vec(),
            b"not SPIR-V but words".to_vec(),
        ];
        for bytes in unreadable {
            assert_eq!(decorate_non_uniform(bytes.clone()), bytes);
        }
    }

    #[test]
    fn what_isnt_spirv_breaks_the_contract_rather_than_passing() {
        let types = Types::of::<[f32; 3], ()>();
        // A header whose bound is past SPIR-V's limit, as no module's is.
        let unbounded = [MAGIC, 0x0001_0500, 0, MOST_BOUND + 1, 0];
        let unbounded: Vec<u8> = unbounded
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .collect();
        for bytes in [
            &b"not SPIR-V but words"[..],
            &[],
            &MAGIC.to_le_bytes(),
            &unbounded,
        ] {
            assert_eq!(
                breaches(bytes, &types),
                ["it isn't SPIR-V the engine can read"]
            );
        }
    }

    /// Types that hold themselves, or that lay out more fields than the check reads, or fields
    /// past what 32-bit offsets reach, as SPIR-V that isn't valid can declare them, break the
    /// contract rather than taking the check's stack, time or memory: as a shader's parameters,
    /// and as the arrays of a resource it binds.
    #[test]
    fn types_without_end_break_the_contract_rather_than_hanging() {
        let types = Types::of::<[f32; 3], [f32; 4]>();
        let module = |instructions: &[Vec<u32>]| {
            let mut words = vec![MAGIC, 0x0001_0500, 0, 64, 0];
            for instruction in instructions {
                let (opcode, operands) = instruction.split_first().expect("an opcode");
                words.push(((1 + operands.len() as u32) << 16) | opcode);
                words.extend_from_slice(operands);
            }
            words
                .iter()
                .flat_map(|word| word.to_le_bytes())
                .collect::<Vec<u8>>()
        };
        // Push constants, struct 12, whose fifth member, at the parameters' place, is the address
        // of type 10, which `types` declare; besides a float, 1, and a uint constant, 3, of
        // `length`.
        let params = offset_of!(Push, params) as u32;
        let breaches_of = |length: u32, types_: &[Vec<u32>]| {
            let mut instructions = vec![
                vec![OP_TYPE_FLOAT, 1, 32],
                vec![OP_TYPE_INT, 2, 32, 0],
                vec![OP_CONSTANT, 2, 3, length],
                vec![OP_TYPE_POINTER, 11, PHYSICAL_STORAGE_BUFFER, 10],
                vec![OP_TYPE_STRUCT, 12, 11, 11, 11, 11, 11],
                vec![OP_MEMBER_DECORATE, 12, 4, OFFSET, params],
                vec![OP_TYPE_POINTER, 13, PUSH_CONSTANT, 12],
                vec![OP_VARIABLE, 13, 14, PUSH_CONSTANT],
            ];
            instructions.extend_from_slice(types_);
            breaches(&module(&instructions), &types)
        };
        let says = |breaches: Vec<String>, way: &str| {
            let said = breaches.iter().any(|breach| breach.contains(way));
            assert!(said, "doesn't say {way:?}: {breaches:#?}");
        };
        let nested = "its parameters, %10, hold types nested more than 64 deep";
        let holds_itself = [
            vec![OP_TYPE_STRUCT, 10, 10],
            vec![OP_MEMBER_DECORATE, 10, 0, OFFSET, 0],
        ];
        says(breaches_of(3, &holds_itself), nested);
        says(breaches_of(3, &[vec![OP_TYPE_ARRAY, 10, 10, 3]]), nested);
        // 2^32 - 1 floats, and three 2 GiB apart.
        says(
            breaches_of(u32::MAX, &[vec![OP_TYPE_ARRAY, 10, 1, 3]]),
            "its parameters, %10, hold more than 1048576 fields",
        );
        let apart = [
            vec![OP_TYPE_ARRAY, 10, 1, 3],
            vec![OP_DECORATE, 10, ARRAY_STRIDE, 1 << 31],
        ];
        says(
            breaches_of(3, &apart),
            "its parameters, %10, hold fields past 4 GiB",
        );
        // Parameters of a float, and a resource of an array that's its own element.
        let resource = [
            vec![OP_TYPE_FLOAT, 10, 32],
            vec![OP_TYPE_ARRAY, 20, 20, 3],
            vec![OP_TYPE_POINTER, 21, UNIFORM_CONSTANT, 20],
            vec![OP_VARIABLE, 21, 22, UNIFORM_CONSTANT],
        ];
        says(
            breaches_of(3, &resource),
            "it binds a resource, %22, outside the descriptor sets",
        );
    }
}
