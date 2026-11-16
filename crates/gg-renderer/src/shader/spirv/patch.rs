//! Patching a shader's SPIR-V: its sampled images decorated `NonUniform`, and zeros as its motion.

use std::collections::{HashMap, HashSet};

use super::{
    FRAGMENT, LOCATION, NON_UNIFORM, OP_CONSTANT_NULL, OP_DECORATE, OP_ENTRY_POINT, OP_FUNCTION,
    OP_FUNCTION_END, OP_RETURN, OP_SAMPLED_IMAGE, OP_STORE, OP_TYPE_POINTER, OP_VARIABLE, OUTPUT,
    instructions, string, words,
};

/// `spirv` with each sampled image made of a `NonUniform` image or sampler decorated `NonUniform`
/// too. Vulkan requires the decoration on the operand reaching the resource, for a sample the
/// sampled image (VUID-RuntimeSpirv-subgroupSize-10149). slangc before Slang 2026.11, the Vulkan
/// SDK 1.4.335's among them, decorates only the image's and sampler's loads
/// (shader-slang/slang#10525). A driver reading the decoration where Vulkan puts it may then sample
/// one instance's texture for a whole subgroup. This can go once the engine requires Slang 2026.11
/// or later, such as the Vulkan SDK 1.4.363's. SPIR-V it can't read stays as it is, for Vulkan to
/// refuse.
pub(in crate::shader) fn decorate_non_uniform(spirv: Vec<u8>) -> Vec<u8> {
    let Some(mut words) = words(&spirv) else {
        return spirv;
    };
    // The IDs decorated NonUniform; where the first such decoration ends, where more go; and each
    // sampled image, with its image and sampler.
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

/// `spirv` with `fragment_main` returning zeros at location 2, the motion, if it returns nothing
/// there.
///
/// Pipelines of depth-writing shaders write the view's motion, that of what owns the depth. A
/// fragment shader returning none would leave what drew behind it, and its pixels would take their
/// history by that; zeros move them with the camera alone. Blending shaders' pipelines don't write
/// it, but lavapipe may (`GraphicsDesc::format` in gg-gpu), blending the zeros as the color blends,
/// which leaves it unchanged.
///
/// The motion is a float4, as the contract has the color at location 0, and takes that output's
/// pointer type. SPIR-V it can't read, or whose fragment shader already returns something at
/// location 2, or nothing at location 0, stays as it is.
pub(in crate::shader) fn return_no_motion(spirv: Vec<u8>) -> Vec<u8> {
    let Some(words) = words(&spirv) else {
        return spirv;
    };
    // The fragment shader's entry point: where it is, its function and interface; each decorated
    // location, with where its decoration ends; each output's pointer type and each pointer's
    // pointee; where the first function starts, and where the entry point's function returns.
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
