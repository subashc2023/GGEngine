//! The SPIR-V slangc writes: checking a shader against the contract that
//! [`Shaders::add`](super::Shaders::add) describes, which Vulkan checks none of, and decorating its
//! sampled images `NonUniform` as Vulkan requires. Both read the module's instructions as the
//! SPIR-V specification lays them out, each a word of its length and opcode, then its operands.

use std::collections::{HashMap, HashSet};
use std::mem::offset_of;

use gg_gpu::SHADER_CAPABILITIES;

use super::Types;
use crate::mesh::{FrameData, Push};

/// SPIR-V's magic number, which opens a module.
const MAGIC: u32 = 0x0723_0203;

/// The most a module's bound, one past its every ID, may be: SPIR-V's universal limit, which
/// spirv-val holds modules to, and which keeps the IDs [`return_no_motion`] adds within a word.
const MOST_BOUND: u32 = 4_194_303;

/// How deep the check follows types nested in types, as structs in structs or arrays of arrays:
/// far past any type laid out as a Rust type is, and short of the stack a type that holds itself,
/// which SPIR-V that isn't valid can declare, would take.
const MOST_NESTED: u32 = 64;

/// The most steps the check takes through a type, a step for each type it meets, each element of
/// an array among them: past any vertex's, parameters' or frame's data's, and short of the time
/// and memory an array of billions of fields would take.
const MOST_STEPS: u32 = 1 << 20;

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

/// What the engine pushes before each draw, a [`Push`]: each member at its offset, in order, but
/// for the word of padding that ends it.
const PUSHED: [(u32, &str, Pushed); 10] = [
    (
        offset_of!(Push, clip_from_world) as u32,
        "the matrix from the world to clip space",
        Pushed::Matrix,
    ),
    (
        offset_of!(Push, vertices) as u32,
        "the vertices' address",
        Pushed::Vertices,
    ),
    (
        offset_of!(Push, transforms) as u32,
        "the instances' transforms' address",
        Pushed::Transforms,
    ),
    (
        offset_of!(Push, ids) as u32,
        "the instances' IDs' address",
        Pushed::Ids,
    ),
    (
        offset_of!(Push, params) as u32,
        "the instances' parameters' address",
        Pushed::Params,
    ),
    (
        offset_of!(Push, view) as u32,
        "the view's size in pixels",
        Pushed::View,
    ),
    (
        offset_of!(Push, frame) as u32,
        "the frame's data's address",
        Pushed::Frame,
    ),
    (
        offset_of!(Push, meshlets) as u32,
        "the address of a draw of meshlets",
        Pushed::Meshlets,
    ),
    (
        offset_of!(Push, draw) as u32,
        "which of the culled runs a draw into the visibility buffer is",
        Pushed::Draw,
    ),
    (
        offset_of!(Push, previous) as u32,
        "how many transforms past each instance's its transform the frame before is",
        Pushed::Previous,
    ),
];

/// The bytes the engine pushes.
const PUSHED_BYTES: usize = size_of::<Push>();

/// What a member of a [`Push`] is.
#[derive(Clone, Copy)]
enum Pushed {
    /// A float4x4.
    Matrix,
    /// The address of the vertices, a `V` each.
    Vertices,
    /// The address of the instances' transforms, a float4x4 each.
    Transforms,
    /// The address of the instances' IDs, a uint2 each.
    Ids,
    /// The address of the instances' parameters, a `P` each.
    Params,
    /// A float2.
    View,
    /// The address of the frame's data, a [`FrameData`].
    Frame,
    /// The address of a draw of meshlets, uint2s, which the model shaders' meshlet geometry reads:
    /// 0 for any other draw.
    Meshlets,
    /// A uint, which the model shaders' meshlets write into the visibility buffer: 0 for any other
    /// draw.
    Draw,
    /// A uint, how many transforms past each instance's its transform the frame before is.
    Previous,
}

/// The capabilities that games' shaders may reach for that need what the engine doesn't enable,
/// named with what in Slang asks for them; others are named by number.
const REFUSED_CAPABILITIES: [(u32, &str); 31] = [
    (9, "Float16, for `half`"),
    (10, "Float64, for `double`"),
    (11, "Int64, for 64-bit integers"),
    (22, "Int16, for 16-bit integers"),
    (39, "Int8, for 8-bit integers"),
    (4433, "StorageBuffer16BitAccess, for 16-bit data"),
    (4435, "StoragePushConstant16, for 16-bit data"),
    (4436, "StorageInputOutput16, for 16-bit varyings"),
    (4448, "StorageBuffer8BitAccess, for 8-bit data"),
    (4450, "StoragePushConstant8, for 8-bit data"),
    (2, "Geometry, for `SV_PrimitiveID`"),
    (69, "ShaderLayer, for `SV_RenderTargetArrayIndex`"),
    (70, "ShaderViewportIndex, for `SV_ViewportArrayIndex`"),
    (4439, "MultiView, for `SV_ViewID`"),
    (52, "InterpolationFunction, for `EvaluateAttribute*`"),
    (25, "ImageGatherExtended, for gathers at offsets that vary"),
    (32, "ClipDistance, for `SV_ClipDistance`"),
    (33, "CullDistance, for `SV_CullDistance`"),
    (34, "ImageCubeArray, for arrays of cube maps"),
    (35, "SampleRateShading, for `SV_SampleIndex`"),
    (41, "SparseResidency, for samples' residency status"),
    (42, "MinLod, for samples with a least level of detail"),
    (61, "GroupNonUniform, for wave operations"),
    (62, "GroupNonUniformVote, for wave operations"),
    (63, "GroupNonUniformArithmetic, for wave operations"),
    (64, "GroupNonUniformBallot, for wave operations"),
    (65, "GroupNonUniformShuffle, for wave operations"),
    (66, "GroupNonUniformShuffleRelative, for wave operations"),
    (68, "GroupNonUniformQuad, for quad operations"),
    (4427, "DrawParameters, for `SV_VertexID` and the like"),
    (5379, "DemoteToHelperInvocation, for demoting fragments"),
];

/// Each way `spirv`, a shader slangc compiled, breaks the contract for a shader of `types`, a
/// line each: none if it keeps to it.
pub(super) fn breaches(spirv: &[u8], types: &Types) -> Vec<String> {
    let Some(module) = Module::read(spirv) else {
        return vec!["it isn't SPIR-V the engine can read".to_owned()];
    };
    let mut breaches = Vec::new();
    module.check_entry_points(&mut breaches);
    module.check_push_constants(types, &mut breaches);
    module.check_resources(&mut breaches);
    module.check_interfaces(&mut breaches);
    module.check_capabilities(&mut breaches);
    module.check_writes(&mut breaches);
    // Whether or not it reads them: each draw's parameters follow the last's, so a size of part
    // of a word would leave every later draw's off the words shaders read them by.
    let (size, rust) = types.params();
    if size % 4 != 0 {
        breaches.push(format!(
            "its type of parameters, {rust}, takes {size} bytes, not a whole number of 4-byte \
             words: each draw's go after the last's, which shaders read by words"
        ));
    }
    breaches
}

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

/// A 32-bit scalar where a type lays it out, and whether it's a float.
struct Scalar {
    at: u32,
    float: bool,
}

/// What [`Module::scalars`] gathers of a type: its 32-bit scalars, and the steps taken, which it
/// counts against [`MOST_STEPS`].
struct Laid {
    /// Whether an address of what's in a buffer is two of its scalars, of 64 bits together, as the
    /// frame's data has them.
    addresses: bool,
    scalars: Vec<Scalar>,
    steps: u32,
}

impl Laid {
    fn new(addresses: bool) -> Self {
        Self {
            addresses,
            scalars: Vec::new(),
            steps: 0,
        }
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

    /// The 32-bit scalars `ty` is made of, from byte `at`, where a matrix's columns are
    /// `matrix_stride` bytes apart, as a struct says of its members, `depth` types deep, gathered
    /// into `laid`; or what in it isn't one, or takes the check past [`MOST_NESTED`] or
    /// [`MOST_STEPS`], or lays out a field past what 32-bit offsets reach.
    fn scalars(
        &self,
        ty: u32,
        at: u32,
        matrix_stride: Option<u32>,
        depth: u32,
        laid: &mut Laid,
    ) -> Result<(), String> {
        laid.steps += 1;
        if laid.steps > MOST_STEPS {
            return Err(format!(
                "more than {MOST_STEPS} fields, past what the check reads"
            ));
        }
        if depth > MOST_NESTED {
            return Err(format!(
                "types nested more than {MOST_NESTED} deep, as one that holds itself is"
            ));
        }
        let depth = depth + 1;
        match self.types.get(&ty) {
            Some(Type::Int { width: 32, .. }) => laid.scalars.push(Scalar { at, float: false }),
            Some(Type::Float { width: 32 }) => laid.scalars.push(Scalar { at, float: true }),
            Some(Type::Int { width, .. }) => return Err(format!("a {width}-bit integer")),
            Some(Type::Float { width }) => return Err(format!("a {width}-bit float")),
            Some(&Type::Vector { component, count }) => {
                for i in 0..count {
                    self.scalars(component, past(at, i, 4)?, None, depth, laid)?;
                }
            }
            Some(&Type::Matrix { column, count }) => {
                let rows = match self.types.get(&column) {
                    Some(&Type::Vector { count, .. }) => count,
                    _ => return Err("a matrix of what isn't vectors".to_owned()),
                };
                let stride = match matrix_stride {
                    Some(stride) => stride,
                    None => past(0, rows, 4)?,
                };
                for i in 0..count {
                    self.scalars(column, past(at, i, stride)?, None, depth, laid)?;
                }
            }
            Some(&Type::Array { element, length }) => {
                let Some(&length) = self.constants.get(&length) else {
                    return Err("an array of a length given as the pipeline's made".to_owned());
                };
                // The first element, at `at`, gives the stride where no decoration does.
                let mut stride = self.decoration(ty, ARRAY_STRIDE);
                for i in 0..length {
                    let first = laid.scalars.len();
                    let at = past(at, i, stride.unwrap_or(0))?;
                    self.scalars(element, at, None, depth, laid)?;
                    stride.get_or_insert(4 * (laid.scalars.len() - first) as u32);
                }
            }
            Some(Type::Struct { members }) => {
                for (member, &member_type) in (0..).zip(members) {
                    let Some(offset) = self.member_decoration(ty, member, OFFSET) else {
                        let name = self.member_name(ty, member);
                        return Err(format!("{name}, laid out nowhere"));
                    };
                    let stride = self.member_decoration(ty, member, MATRIX_STRIDE);
                    self.scalars(member_type, past(at, 1, offset)?, stride, depth, laid)?;
                }
            }
            Some(Type::RuntimeArray { .. }) => {
                return Err("an array without a length, which no Rust type is".to_owned());
            }
            Some(&Type::Pointer { storage, .. })
                if laid.addresses && storage == PHYSICAL_STORAGE_BUFFER =>
            {
                laid.scalars.push(Scalar { at, float: false });
                laid.scalars.push(Scalar {
                    at: past(at, 1, 4)?,
                    float: false,
                });
            }
            Some(Type::Pointer { .. }) => return Err("an address".to_owned()),
            Some(Type::Bool) => return Err("a bool".to_owned()),
            _ => return Err("what isn't data".to_owned()),
        }
        Ok(())
    }

    /// The size of what `ty` lays out from byte 0, if it's 32-bit scalars back to back, addresses
    /// among them if `addresses`, as [`scalars`](Self::scalars) says; or why it isn't, as what it
    /// holds.
    fn packed_size(&self, ty: u32, addresses: bool) -> Result<u32, String> {
        let mut laid = Laid::new(addresses);
        self.scalars(ty, 0, None, 0, &mut laid)
            .map_err(|what| format!("hold {what}"))?;
        packed(&laid.scalars)
    }

    /// Whether `ty`, where a struct lays out its columns `matrix_stride` bytes apart, is 16
    /// floats back to back: a float4x4, as slangc lays one out.
    fn is_float4x4(&self, ty: u32, matrix_stride: Option<u32>) -> bool {
        let mut laid = Laid::new(false);
        let laid_out = self.scalars(ty, 0, matrix_stride, 0, &mut laid).is_ok();
        let scalars = &laid.scalars;
        laid_out
            && scalars.len() == 16
            && packed(scalars) == Ok(64)
            && scalars.iter().all(|scalar| scalar.float)
    }

    fn check_entry_points(&self, breaches: &mut Vec<String>) {
        let has = |model, name| {
            (self.entry_points.iter()).any(|entry| entry.model == model && entry.name == name)
        };
        if self.entry_points.len() != 2
            || !has(VERTEX, "vertex_main")
            || !has(FRAGMENT, "fragment_main")
        {
            let names: Vec<&str> = (self.entry_points.iter())
                .map(|entry| entry.name.as_str())
                .collect();
            breaches.push(format!(
                "its entry points are {}, where a shader has `vertex_main`, a vertex shader, and \
                 `fragment_main`, a fragment shader",
                names.join(", ")
            ));
        }
    }

    fn check_push_constants(&self, types: &Types, breaches: &mut Vec<String>) {
        let blocks: Vec<&Variable> = (self.variables.iter())
            .filter(|variable| variable.storage == PUSH_CONSTANT)
            .collect();
        // slangc leaves them out of a shader that reads none.
        let block = match blocks[..] {
            [] => return,
            [block] => block,
            _ => {
                breaches.push(format!(
                    "it has {} blocks of push constants, where the engine pushes one",
                    blocks.len()
                ));
                return;
            }
        };
        let block = self.pointee(block.ty, PUSH_CONSTANT);
        let Some((block, Type::Struct { members })) =
            block.and_then(|block| Some((block, self.types.get(&block)?)))
        else {
            breaches.push("its push constants aren't a struct".to_owned());
            return;
        };
        for (member, &ty) in (0..).zip(members) {
            let name = self.member_name(block, member);
            let offset = self.member_decoration(block, member, OFFSET);
            let Some(&(pushed_at, pushed, kind)) = PUSHED.get(member as usize) else {
                breaches.push(format!(
                    "it reads {name} from its push constants, past the {PUSHED_BYTES} bytes the \
                     engine pushes"
                ));
                continue;
            };
            if offset != Some(pushed_at) {
                let at = offset.map_or_else(|| "nowhere".to_owned(), |at| format!("byte {at}"));
                breaches.push(format!(
                    "it reads {name} from its push constants at {at}, where the engine pushes \
                     {pushed} at byte {pushed_at}"
                ));
                continue;
            }
            match kind {
                Pushed::Matrix => {
                    let stride = self.member_decoration(block, member, MATRIX_STRIDE);
                    if !self.is_float4x4(ty, stride) {
                        breaches.push(format!(
                            "its push constants' {name} isn't a float4x4, where the engine \
                             pushes {pushed}"
                        ));
                    }
                }
                Pushed::Vertices => {
                    self.check_pointee(ty, "vertices", "vertex type", types.vertex(), breaches);
                }
                Pushed::Transforms => {
                    let transforms = self.pointee(ty, PHYSICAL_STORAGE_BUFFER);
                    let stride = self.decoration(ty, ARRAY_STRIDE);
                    let float4x4 = transforms.is_some_and(|ty| self.is_float4x4(ty, None));
                    if !float4x4 || stride.is_some_and(|stride| stride != 64) {
                        breaches.push(format!(
                            "its push constants' {name} isn't the address of float4x4s, where \
                             the engine pushes {pushed}"
                        ));
                    }
                }
                // Not a struct: a P* params of 8 bytes declared where the IDs go would read as
                // their address otherwise.
                Pushed::Ids | Pushed::Meshlets => {
                    let pointee = self.pointee(ty, PHYSICAL_STORAGE_BUFFER);
                    let stride = self.decoration(ty, ARRAY_STRIDE);
                    let uint2 = pointee.is_some_and(|ty| self.signed_int2(ty) == Some(false));
                    if !uint2 || stride.is_some_and(|stride| stride != 8) {
                        breaches.push(format!(
                            "its push constants' {name} isn't the address of uint2s, where the \
                             engine pushes {pushed}"
                        ));
                    }
                }
                Pushed::Params if types.params_size == 0 => breaches.push(format!(
                    "it reads parameters through {name}, but it's added without them, `()`"
                )),
                Pushed::Params => self.check_pointee(
                    ty,
                    "parameters",
                    "type of parameters",
                    types.params(),
                    breaches,
                ),
                Pushed::View => {
                    let float2 = matches!(self.types.get(&ty), Some(&Type::Vector { component, count: 2 })
                        if self.is_float(component));
                    if !float2 {
                        breaches.push(format!(
                            "its push constants' {name} isn't a float2, where the engine pushes \
                             {pushed}"
                        ));
                    }
                }
                Pushed::Frame => self.check_frame(ty, breaches),
                Pushed::Draw | Pushed::Previous => {
                    let uint = matches!(
                        self.types.get(&ty),
                        Some(Type::Int {
                            width: 32,
                            signed: false
                        })
                    );
                    if !uint {
                        breaches.push(format!(
                            "its push constants' {name} isn't a uint, where the engine pushes \
                             {pushed}"
                        ));
                    }
                }
            }
        }
    }

    /// Checks that `pointer`, the type of the push constant with the address of the frame's data,
    /// points at the start of what [`FrameData`] lays out: no more bytes, of 32-bit fields and
    /// addresses without padding, so that a shader that reads the fields before those the engine
    /// adds to its end keeps to the contract.
    fn check_frame(&self, pointer: u32, breaches: &mut Vec<String>) {
        let size = size_of::<FrameData>();
        let Some(pointee) = self.pointee(pointer, PHYSICAL_STORAGE_BUFFER) else {
            breaches.push(
                "its push constants hold the frame's data's address as what isn't an address"
                    .to_owned(),
            );
            return;
        };
        let name = self.name(pointee);
        match self.packed_size(pointee, true) {
            Ok(found) if found as usize <= size => {}
            Ok(found) => breaches.push(format!(
                "its frame's data, {name}, take {found} bytes, past the {size} the engine's take"
            )),
            Err(why) => breaches.push(format!(
                "its frame's data, {name}, {why}, where the engine's have 32-bit fields and \
                 addresses alone, without padding"
            )),
        }
    }

    /// Checks that `pointer`, the type of a push constant with the address of the shader's
    /// `what`, points at what `rust`, its Rust type named `role`, lays out: as many bytes, of
    /// 32-bit fields without padding.
    fn check_pointee(
        &self,
        pointer: u32,
        what: &str,
        role: &str,
        (size, rust): (usize, &str),
        breaches: &mut Vec<String>,
    ) {
        let Some(pointee) = self.pointee(pointer, PHYSICAL_STORAGE_BUFFER) else {
            breaches.push(format!(
                "its push constants hold its {what} as what isn't an address"
            ));
            return;
        };
        let name = self.name(pointee);
        let found = self.packed_size(pointee, false).and_then(|found| {
            match self.decoration(pointer, ARRAY_STRIDE) {
                Some(stride) if stride != found => Err(format!(
                    "take {stride} bytes each, {} of them padding",
                    stride.saturating_sub(found)
                )),
                _ => Ok(found),
            }
        });
        match found {
            Ok(found) if found as usize == size => {}
            Ok(found) => breaches.push(format!(
                "its {what}, {name}, take {found} bytes, where its {role}, {rust}, takes {size}"
            )),
            Err(why) => breaches.push(format!(
                "its {what}, {name}, {why}, where its {role}, {rust}, has 32-bit fields alone, \
                 without padding"
            )),
        }
    }

    fn check_resources(&self, breaches: &mut Vec<String>) {
        let resources = [UNIFORM_CONSTANT, UNIFORM, STORAGE_BUFFER];
        for variable in &self.variables {
            if !resources.contains(&variable.storage) {
                continue;
            }
            let set = self.decoration(variable.id, DESCRIPTOR_SET);
            let binding = self.decoration(variable.id, BINDING);
            let pointee = self.pointee(variable.ty, variable.storage);
            let element = pointee.and_then(|array| match self.types.get(&array) {
                Some(&Type::RuntimeArray { element }) => self.types.get(&element),
                _ => None,
            });
            let bindless = match (set, binding, element) {
                (Some(0), Some(0), Some(Type::Image(image))) => {
                    image.is_texture() && self.is_float(image.sampled)
                }
                (Some(0), Some(1), Some(Type::Sampler)) => true,
                _ => false,
            };
            if bindless && variable.storage == UNIFORM_CONSTANT {
                continue;
            }
            let at = match (set, binding) {
                (Some(set), Some(binding)) => format!("at set {set}, binding {binding}"),
                _ => "outside the descriptor sets".to_owned(),
            };
            breaches.push(format!(
                "it binds {}, {}, {at}, where a shader binds only the bindless set's \
                 `Texture2D textures[]` or `TextureCube cubes[]` at set 0, binding 0, and \
                 `SamplerState samplers[]` or `SamplerComparisonState comparisons[]` at binding 1",
                self.resource(variable),
                self.name(variable.id)
            ));
        }
    }

    /// What a resource the shader binds is, for a breach to name.
    fn resource(&self, variable: &Variable) -> &'static str {
        let pointee = self.pointee(variable.ty, variable.storage);
        if variable.storage == STORAGE_BUFFER
            || pointee.is_some_and(|block| self.decoration(block, BUFFER_BLOCK).is_some())
        {
            return "a buffer";
        }
        if variable.storage == UNIFORM {
            return "a constant buffer";
        }
        // An array's elements, as deep as the check follows them: of one that holds itself, they
        // would go on for good.
        let mut ty = pointee;
        for _ in 0..MOST_NESTED {
            match ty.and_then(|ty| self.types.get(&ty)) {
                Some(&(Type::Array { element, .. } | Type::RuntimeArray { element })) => {
                    ty = Some(element);
                }
                _ => break,
            }
        }
        match ty.and_then(|ty| self.types.get(&ty)) {
            Some(Type::Image(image)) if image.usage == 2 => "a storage image",
            Some(Type::Image(image)) => match self.types.get(&image.sampled) {
                Some(Type::Float { width: 32 }) => "textures",
                Some(Type::Int { .. }) => "textures of integers",
                _ => "textures of what isn't 32-bit floats",
            },
            Some(Type::Sampler) => "samplers",
            Some(Type::SampledImage) => "combined textures and samplers",
            _ => "a resource",
        }
    }

    fn check_interfaces(&self, breaches: &mut Vec<String>) {
        let variables: HashMap<u32, &Variable> = (self.variables.iter())
            .map(|variable| (variable.id, variable))
            .collect();
        for entry in &self.entry_points {
            let interface = (entry.interface.iter()).filter_map(|id| variables.get(id).copied());
            match entry.model {
                VERTEX => {
                    for input in interface.filter(|variable| variable.storage == INPUT) {
                        self.check_vertex_input(input, breaches);
                    }
                }
                FRAGMENT => {
                    let outputs: Vec<&Variable> = interface
                        .filter(|variable| variable.storage == OUTPUT)
                        .collect();
                    self.check_fragment_outputs(&outputs, breaches);
                }
                _ => {}
            }
        }
    }

    fn check_vertex_input(&self, input: &Variable, breaches: &mut Vec<String>) {
        let name = (self.interface_name(input.id)).unwrap_or_else(|| "a value".to_owned());
        // Draw parameters, which need a feature.
        let parameter = match self.decoration(input.id, BUILT_IN) {
            Some(VERTEX_INDEX | INSTANCE_INDEX) => return,
            Some(BASE_VERTEX) => "BaseVertex, as `SV_VertexID` and `SV_StartVertexLocation` do",
            Some(BASE_INSTANCE) => "BaseInstance, as `SV_InstanceID` does",
            Some(DRAW_INDEX) => "DrawIndex",
            Some(built_in) => {
                breaches.push(format!(
                    "its vertex stage reads built-in {built_in}, {name}, where it reads only \
                     `SV_VulkanVertexID` and `SV_VulkanInstanceID`"
                ));
                return;
            }
            None => {
                let location = self.decoration(input.id, LOCATION);
                let at = location.map_or_else(String::new, |at| format!(" at location {at}"));
                breaches.push(format!(
                    "its vertex stage reads {name}{at} as a vertex attribute, which the engine \
                     gives nothing; it reads its vertices through their address"
                ));
                return;
            }
        };
        breaches.push(format!(
            "its vertex stage reads {parameter}, which needs a feature the engine doesn't \
             enable; it reads `SV_VulkanVertexID` and `SV_VulkanInstanceID`"
        ));
    }

    fn check_fragment_outputs(&self, outputs: &[&Variable], breaches: &mut Vec<String>) {
        let (mut color, mut id) = (false, false);
        let float4 = |ty: Option<u32>| {
            ty.and_then(|ty| self.types.get(&ty)).is_some_and(|ty| {
                matches!(ty, &Type::Vector { component, count: 4 } if self.is_float(component))
            })
        };
        for output in outputs {
            let name = self.interface_name(output.id);
            // ", color," of a field it returns, or nothing of what it returns whole.
            let named = (name.as_ref()).map_or_else(String::new, |name| format!(", {name},"));
            let location = self.decoration(output.id, LOCATION);
            let ty = self.pointee(output.ty, OUTPUT);
            match (self.decoration(output.id, BUILT_IN), location) {
                (None, Some(0)) => {
                    color = true;
                    if !float4(ty) {
                        breaches.push(format!(
                            "its fragment stage returns its color{named} as what isn't a float4"
                        ));
                    }
                }
                (None, Some(2)) => {
                    if !float4(ty) {
                        breaches.push(format!(
                            "its fragment stage returns its motion{named} as what isn't a float4"
                        ));
                    }
                }
                (None, Some(1)) => {
                    id = true;
                    match ty.and_then(|ty| self.signed_int2(ty)) {
                        Some(false) => {}
                        Some(true) => breaches.push(format!(
                            "its fragment stage writes its ID{named} as int2: write uint2, as \
                             the view's IDs are unsigned"
                        )),
                        None => breaches.push(format!(
                            "its fragment stage writes its ID{named} as what isn't a uint2"
                        )),
                    }
                }
                (None, location) => {
                    let what = name.unwrap_or_else(|| "its color".to_owned());
                    let at = location.map_or_else(String::new, |at| format!(" at location {at}"));
                    breaches.push(format!(
                        "its fragment stage writes {what}{at}, where the engine draws into three \
                         targets: the color at location 0, the ID at location 1, and the motion \
                         at location 2"
                    ));
                }
                (Some(FRAG_DEPTH), _) => breaches.push(
                    "its fragment stage writes its depth, `SV_Depth`, which the engine leaves to \
                     the rasterizer"
                        .to_owned(),
                ),
                (Some(SAMPLE_MASK), _) => breaches.push(
                    "its fragment stage writes its coverage, `SV_Coverage`, which the engine \
                     leaves to the rasterizer"
                        .to_owned(),
                ),
                (Some(built_in), _) => breaches.push(format!(
                    "its fragment stage writes built-in {built_in}{named} where it returns only \
                     its color, its ID and its motion"
                )),
            }
        }
        if !color {
            breaches.push("its fragment stage returns no color at location 0".to_owned());
        }
        if !id {
            breaches.push(
                "its fragment stage writes no entity ID at location 1: beside its color it returns \
                 its draw's ID, a uint2, its instance's, read through `uint2* ids` in its push \
                 constants, before `P* params`, or 0 where it draws no entity"
                    .to_owned(),
            );
        }
    }

    fn check_capabilities(&self, breaches: &mut Vec<String>) {
        for &capability in &self.capabilities {
            if SHADER_CAPABILITIES.contains(&capability) {
                continue;
            }
            let named = REFUSED_CAPABILITIES.iter().find(|&&(n, _)| n == capability);
            let name = named.map_or_else(|| capability.to_string(), |&(_, name)| name.to_owned());
            breaches.push(format!(
                "it uses SPIR-V's capability {name}, which needs a feature the engine doesn't \
                 enable"
            ));
        }
    }

    fn check_writes(&self, breaches: &mut Vec<String>) {
        let through_address = self.written.iter().any(|pointer| {
            (self.value_types.get(pointer))
                .and_then(|ty| self.pointee(*ty, PHYSICAL_STORAGE_BUFFER))
                .is_some()
        });
        if through_address {
            breaches.push(
                "it writes through an address the engine pushes, such as its vertices', which \
                 shaders only read: writing from these stages needs features the engine doesn't \
                 enable"
                    .to_owned(),
            );
        }
    }
}

/// The size of `scalars` if they're back to back from byte 0, as a struct without padding lays
/// them out; or where the first gap is.
fn packed(scalars: &[Scalar]) -> Result<u32, String> {
    for (i, scalar) in (0..).zip(scalars) {
        if scalar.at != 4 * i {
            return Err(format!("have padding before byte {}", scalar.at.max(4 * i)));
        }
    }
    Ok(4 * scalars.len() as u32)
}

/// Byte `at` and `count` more of `size` bytes past it, if 32-bit offsets reach that far; or what
/// a field laid out there is, for a breach to name.
fn past(at: u32, count: u32, size: u32) -> Result<u32, String> {
    (count.checked_mul(size))
        .and_then(|bytes| at.checked_add(bytes))
        .ok_or_else(|| "fields past 4 GiB".to_owned())
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

/// `spirv`, a shader slangc compiled, with each sampled image that it makes of an image or a
/// sampler decorated `NonUniform` decorated so too. Vulkan requires the decoration on the operand
/// an instruction reaches a resource through, which for a sample is the sampled image
/// (VUID-RuntimeSpirv-subgroupSize-10149), but slangc before Slang 2026.11, the Vulkan SDK
/// 1.4.335's among them, decorates only the loads of the image and the sampler
/// (shader-slang/slang#10525): a driver that reads the decoration where Vulkan puts it may sample
/// one instance's texture for every instance of a subgroup. This can go once the engine requires
/// a slangc that decorates it, Slang 2026.11's or later, such as the Vulkan SDK 1.4.363's. SPIR-V
/// it can't read stays as it is, for Vulkan to refuse.
pub(super) fn decorate_non_uniform(spirv: Vec<u8>) -> Vec<u8> {
    let Some(mut words) = words(&spirv) else {
        return spirv;
    };
    // The IDs decorated NonUniform; where the first of those decorations ends, which is where
    // more go; and each sampled image, with the image and the sampler it's made of.
    let mut non_uniform = HashSet::new();
    let mut end = None;
    let mut sampled = Vec::new();
    for instruction in instructions(&words) {
        let Some((at, opcode, operands)) = instruction else {
            return spirv;
        };
        match (opcode, operands) {
            (OP_DECORATE, &[id, NON_UNIFORM]) => {
                non_uniform.insert(id);
                end.get_or_insert(at + 1 + operands.len());
            }
            (OP_SAMPLED_IMAGE, &[_, id, image, sampler]) => sampled.push([id, image, sampler]),
            _ => {}
        }
    }
    let decorations: Vec<u32> = sampled
        .into_iter()
        .filter(|[id, image, sampler]| {
            !non_uniform.contains(id)
                && (non_uniform.contains(image) || non_uniform.contains(sampler))
        })
        .flat_map(|[id, ..]| [(3 << 16) | OP_DECORATE, id, NON_UNIFORM])
        .collect();
    let Some(end) = end.filter(|_| !decorations.is_empty()) else {
        return spirv;
    };
    words.splice(end..end, decorations);
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

/// `spirv`, a shader that writes depth slangc compiled, its fragment shader, `fragment_main`,
/// returning zeros at location 2, as the motion of what it draws, if it returns nothing there. The
/// pipelines of shaders that write depth write the view's motion, the motion of what owns the
/// depth: one whose fragment shader returned none would leave there what drew behind it, whose
/// motion its pixels would then be taken back by, where zeros move them with the camera alone. The
/// motion is a float4, the type of the color at location 0, which the contract has a float4, whose
/// output's pointer type the motion's takes too. SPIR-V it can't read, or whose fragment shader
/// returns something at location 2 already, or nothing at location 0, stays as it is.
pub(super) fn return_no_motion(spirv: Vec<u8>) -> Vec<u8> {
    let Some(words) = words(&spirv) else {
        return spirv;
    };
    // The fragment shader's entry point, where it is, its function and its interface; each
    // location decorated, with where its decoration ends; each output's pointer type, and each
    // pointer's pointee; where the first function starts, and the entry point's function returns.
    let mut entry = None;
    let mut locations = HashMap::new();
    let (mut outputs, mut pointees) = (HashMap::new(), HashMap::new());
    let mut first_function = None;
    let (mut returns, mut in_entry) = (Vec::new(), false);
    for instruction in instructions(&words) {
        let Some((at, opcode, operands)) = instruction else {
            return spirv;
        };
        match (opcode, operands) {
            (OP_ENTRY_POINT, &[FRAGMENT, function, ref named @ ..]) => {
                let Some((name, taken)) = string(named) else {
                    return spirv;
                };
                if name == "fragment_main" {
                    entry = Some((at, function, named[taken..].to_vec()));
                }
            }
            (OP_DECORATE, &[id, LOCATION, location]) => {
                locations.insert(id, (location, at + 1 + operands.len()));
            }
            (OP_TYPE_POINTER, &[id, OUTPUT, pointee]) => {
                pointees.insert(id, pointee);
            }
            (OP_VARIABLE, &[ty, id, OUTPUT, ..]) => {
                outputs.insert(id, ty);
            }
            (OP_FUNCTION, &[_, id, ..]) => {
                first_function.get_or_insert(at);
                in_entry = entry
                    .as_ref()
                    .is_some_and(|&(_, function, _)| function == id);
            }
            (OP_FUNCTION_END, _) => in_entry = false,
            (OP_RETURN, _) if in_entry => returns.push(at),
            _ => {}
        }
    }
    let (Some((entry_at, _, interface)), Some(first_function)) = (entry, first_function) else {
        return spirv;
    };
    let output = |location| {
        let at = |id: &&u32| locations.get(*id).map(|&(at, _)| at) == Some(location);
        (interface.iter())
            .filter(|id| outputs.contains_key(*id))
            .find(at)
            .copied()
    };
    let Some(color) = output(0).filter(|_| output(2).is_none()) else {
        return spirv;
    };
    let pointer = outputs[&color];
    let Some(&float4) = pointees.get(&pointer) else {
        return spirv;
    };
    let (_, decorated) = locations[&color];
    // Two new IDs: the zeros, and the motion's variable.
    let (zeros, motion) = (words[3], words[3] + 1);
    let mut patched = Vec::with_capacity(words.len() + 16);
    patched.extend_from_slice(&words[..5]);
    patched[3] = motion + 1;
    for instruction in instructions(&words) {
        let (at, opcode, operands) = instruction.expect("read once already");
        let end = at + 1 + operands.len();
        if at == first_function {
            patched.extend([(3 << 16) | OP_CONSTANT_NULL, float4, zeros]);
            patched.extend([(4 << 16) | OP_VARIABLE, pointer, motion, OUTPUT]);
        }
        if returns.contains(&at) {
            patched.extend([(3 << 16) | OP_STORE, motion, zeros]);
        }
        if at == entry_at {
            // Its interface lists every global variable it uses, the motion's among them.
            patched.push((((end - at + 1) as u32) << 16) | opcode);
            patched.extend_from_slice(operands);
            patched.push(motion);
        } else {
            patched.extend_from_slice(&words[at..end]);
        }
        if end == decorated {
            patched.extend([(4 << 16) | OP_DECORATE, motion, LOCATION, 2]);
        }
    }
    patched.iter().flat_map(|word| word.to_le_bytes()).collect()
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
    use super::*;

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
