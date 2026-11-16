//! Checking a shader against the contract games' shaders keep, which Vulkan doesn't check.

use std::collections::HashMap;
use std::mem::offset_of;

use gg_gpu::SHADER_CAPABILITIES;

use super::{
    ARRAY_STRIDE, BASE_INSTANCE, BASE_VERTEX, BINDING, BUFFER_BLOCK, BUILT_IN, DESCRIPTOR_SET,
    DRAW_INDEX, FRAG_DEPTH, FRAGMENT, INPUT, INSTANCE_INDEX, LOCATION, MATRIX_STRIDE, Module,
    OFFSET, OUTPUT, PHYSICAL_STORAGE_BUFFER, PUSH_CONSTANT, SAMPLE_MASK, STORAGE_BUFFER, Type,
    UNIFORM, UNIFORM_CONSTANT, VERTEX, VERTEX_INDEX, Variable,
};
use crate::mesh::{FrameData, Push};
use crate::shader::Types;

/// How deep the check follows nested types: far past any type laid out as a Rust type, and short of
/// the stack a self-holding type (invalid SPIR-V can declare one) would take.
const MOST_NESTED: u32 = 64;

/// The most steps the check takes through a type, one per type met, each array element among them:
/// past any vertex, parameters or frame data, and short of the time and memory billions of fields
/// would take.
const MOST_STEPS: u32 = 1 << 20;

/// Each member of a [`Push`] at its offset, in order, less the final padding word.
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

const PUSHED_BYTES: usize = size_of::<Push>();

/// What a member of a [`Push`] is.
#[derive(Clone, Copy)]
enum Pushed {
    /// A float4x4.
    Matrix,
    /// The vertices' address, a `V` each.
    Vertices,
    /// The transforms' address, a float4x4 per instance.
    Transforms,
    /// The IDs' address, a uint2 per instance.
    Ids,
    /// The parameters' address, a `P` per instance.
    Params,
    /// A float2.
    View,
    /// The address of the frame's [`FrameData`].
    Frame,
    /// The address of a meshlet draw's uint2s, read by the model shaders' meshlet geometry; 0 for
    /// any other draw.
    Meshlets,
    /// A uint the model shaders' meshlets write into the visibility buffer; 0 for any other draw.
    Draw,
    /// A uint: how many transforms past each instance's its last frame's is.
    Previous,
}

/// Capabilities games' shaders may reach for that need what the engine doesn't enable, named by
/// what in Slang asks for them. Others are named by number.
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

/// Each way `spirv`, as slangc compiled it, breaks the contract for `types`, a line each. Empty if
/// it keeps to it.
pub(in crate::shader) fn breaches(spirv: &[u8], types: &Types) -> Vec<String> {
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
    // Read or not: each draw's parameters follow the last's, so part of a word would put every
    // later draw's off the words shaders read them by.
    let (size, rust) = types.params();
    if size % 4 != 0 {
        breaches.push(format!(
            "its type of parameters, {rust}, takes {size} bytes, not a whole number of 4-byte \
             words: each draw's go after the last's, which shaders read by words"
        ));
    }
    breaches
}

/// A 32-bit scalar where a type lays it out, and whether it's a float.
struct Scalar {
    at: u32,
    float: bool,
}

/// What [`Module::scalars`] gathers of a type: its 32-bit scalars, and the steps taken, counted
/// against [`MOST_STEPS`].
struct Laid {
    /// Whether an address in a buffer counts as two 32-bit scalars, as the frame's data has them.
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

impl Module {
    /// Gathers into `laid` the 32-bit scalars `ty` is made of, from byte `at`, `depth` types deep,
    /// a matrix's columns `matrix_stride` bytes apart as a struct says. Errs with what in it isn't
    /// one, or takes the check past [`MOST_NESTED`] or [`MOST_STEPS`], or lies past 32-bit offsets.
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

    /// The size `ty` lays out from byte 0, if it's 32-bit scalars back to back, addresses among
    /// them if `addresses` ([`scalars`](Self::scalars)); or why not, naming what it holds.
    fn packed_size(&self, ty: u32, addresses: bool) -> Result<u32, String> {
        let mut laid = Laid::new(addresses);
        self.scalars(ty, 0, None, 0, &mut laid)
            .map_err(|what| format!("hold {what}"))?;
        packed(&laid.scalars)
    }

    /// Whether `ty`, its columns `matrix_stride` bytes apart, is 16 floats back to back: a float4x4
    /// as slangc lays one out.
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
                // Not a struct: otherwise 8-byte `P* params` declared where the IDs go would read
                // as their address.
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

    /// Checks that `pointer`, the frame data push constant's type, points at a prefix of what
    /// [`FrameData`] lays out: no more bytes, of 32-bit fields and addresses without padding. So a
    /// shader reading the fields before those the engine adds at its end keeps to the contract.
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

    /// Checks that `pointer`, the type of a push constant addressing the shader's `what`, points at
    /// what `rust` (its Rust type, named `role`) lays out: as many bytes, of 32-bit fields without
    /// padding.
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

    /// What a bound resource is, for a breach to name.
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
        // An array's elements, as deep as the check follows them: a self-holding one's would go on
        // for good.
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
            // ", color," for a returned field, nothing for what it returns whole.
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

/// The size of `scalars` if they're back to back from byte 0, as in an unpadded struct; or where
/// the first gap is.
fn packed(scalars: &[Scalar]) -> Result<u32, String> {
    for (i, scalar) in (0..).zip(scalars) {
        if scalar.at != 4 * i {
            return Err(format!("have padding before byte {}", scalar.at.max(4 * i)));
        }
    }
    Ok(4 * scalars.len() as u32)
}

/// Byte `at` plus `count` times `size`, if 32-bit offsets reach it; or what a field there is, for a
/// breach to name.
fn past(at: u32, count: u32, size: u32) -> Result<u32, String> {
    (count.checked_mul(size))
        .and_then(|bytes| at.checked_add(bytes))
        .ok_or_else(|| "fields past 4 GiB".to_owned())
}
