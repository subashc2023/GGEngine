//! What a script's compiled bytecode shows of it: whether one of its functions assigns a local of
//! its chunk, which [`Scripts`](super::Scripts) refuses. It reads the bytecode as Luau's loader does,
//! `luau_load` in luau0-src's `VM/src/lvmload.cpp`, the opcodes, their lengths and the capture
//! types as `Common/include/Luau/Bytecode.h` and `BytecodeUtils.h` give them: a Luau upgrade that
//! changes them fails the script tests, which every script's load reads through here.

/// Opcodes, by their values in `LuauOpcode`.
const SETUPVAL: u8 = 10;
const NEWCLOSURE: u8 = 19;
const DUPCLOSURE: u8 = 64;
const CAPTURE: u8 = 70;

/// The opcodes followed by an auxiliary word, as `getOpLength` says: GETGLOBAL, SETGLOBAL,
/// GETIMPORT, GETTABLEKS, SETTABLEKS and NAMECALL; the six JUMPIFs from JUMPIFEQ; NEWTABLE,
/// SETLIST, FORGLOOP, FASTCALL3, LOADKX, FASTCALL2 and FASTCALL2K; the four JUMPXEQKs; and
/// GETUDATAKS, SETUDATAKS, NAMECALLUDATA, NEWCLASSMEMBER, CALLFB, CMPPROTO and NEWCLASS.
const WITH_AUX: [u8; 30] = [
    7, 8, 12, 15, 16, 20, 27, 28, 29, 30, 31, 32, 53, 55, 58, 60, 66, 74, 75, 77, 78, 79, 80, 83,
    84, 85, 86, 87, 88, 90,
];

/// A capture's types, by their values in `LuauCaptureType`: of a register, by value or by
/// reference, or of the function's own upvalue.
const CAPTURE_UPVAL: u8 = 2;

/// Constants' kinds, by their values in `LuauBytecodeTag`.
mod constant {
    pub(super) const NIL: u8 = 0;
    pub(super) const BOOLEAN: u8 = 1;
    pub(super) const NUMBER: u8 = 2;
    pub(super) const STRING: u8 = 3;
    pub(super) const IMPORT: u8 = 4;
    pub(super) const TABLE: u8 = 5;
    pub(super) const CLOSURE: u8 = 6;
    pub(super) const VECTOR: u8 = 7;
    pub(super) const TABLE_WITH_CONSTANTS: u8 = 8;
    pub(super) const INTEGER: u8 = 9;
    pub(super) const CLASS_SHAPE: u8 = 10;
    pub(super) const VECTORD: u8 = 11;
}

/// The bytecode versions the loader takes, `LBC_VERSION_MIN` to `LBC_VERSION_MAX`.
const VERSIONS: std::ops::RangeInclusive<u8> = 3..=14;

/// Where a function of a chunk assigns a local of the chunk: the line, and the local's name.
#[derive(Debug, PartialEq)]
pub(super) struct Assigned {
    pub(super) line: u32,
    pub(super) local: String,
}

/// A function of the bytecode, as much as the scan reads of it.
struct Proto {
    upvalues: usize,
    code: Vec<u32>,
    /// The functions its closure constants make, each its constant's index and the function's.
    closures: Vec<(usize, usize)>,
    /// The functions it makes, by their index among its own.
    children: Vec<usize>,
    /// Its upvalues' names, as indexes of the string table from 1, 0 for none.
    names: Vec<u32>,
    /// Its instructions' lines: the gap's log2, each's offset and each gap's line.
    lines: Option<(u8, Vec<u8>, Vec<i32>)>,
}

impl Proto {
    /// The line of instruction `pc`, as `luaG_getline` reads it.
    fn line(&self, pc: usize) -> u32 {
        self.lines.as_ref().map_or(0, |(gap, offsets, lines)| {
            let line = lines[pc >> gap] + i32::from(offsets[pc]);
            u32::try_from(line).unwrap_or(0)
        })
    }
}

/// The first place a function of the chunk that `bytecode` holds, as Luau's compiler wrote it,
/// assigns a local of the chunk, by an upvalue that is it or that a function between them took from
/// it: `None` if none does. The chunk's own code may assign them, as it runs once, and a function
/// may assign its own locals, and its enclosing functions', but for the chunk's.
///
/// Fails, saying why, for bytecode it can't read, which no compiler of the Luau the engine builds
/// writes.
pub(super) fn assigned(bytecode: &[u8]) -> Result<Option<Assigned>, String> {
    let mut read = Reader {
        bytes: bytecode,
        at: 0,
    };
    let version = read.byte()?;
    if !VERSIONS.contains(&version) {
        return Err(format!(
            "Luau's bytecode of version {version}, which the engine doesn't read"
        ));
    }
    let types = if version >= 4 { read.byte()? } else { 0 };
    let strings = read.var()?;
    let mut names = Vec::new();
    for _ in 0..strings {
        let length = read.var()? as usize;
        names.push(String::from_utf8_lossy(read.bytes(length)?).into_owned());
    }
    if types == 3 {
        while read.byte()? != 0 {
            read.var()?;
        }
    }
    let count = read.var()? as usize;
    let mut protos = Vec::new();
    for _ in 0..count {
        let size = if version >= 12 {
            Some(read.var()? as usize)
        } else {
            None
        };
        let start = read.at;
        protos.push(read.proto(version, types)?);
        if let Some(size) = size {
            read.at = start + size;
        }
    }
    let main = read.var()? as usize;
    // Whether each upvalue of each function is a local of the chunk, found as the function that
    // makes it is scanned, from the chunk down.
    let mut chunk_locals: Vec<Option<Vec<bool>>> = (0..protos.len()).map(|_| None).collect();
    let main_proto = protos.get(main).ok_or("no chunk's function")?;
    chunk_locals[main] = Some(vec![false; main_proto.upvalues]);
    let mut scan = vec![main];
    while let Some(parent) = scan.pop() {
        let proto = &protos[parent];
        let mut pc = 0;
        while let Some(&insn) = proto.code.get(pc) {
            let op = insn as u8;
            if op == NEWCLOSURE || op == DUPCLOSURE {
                let d = (insn as i32 >> 16) as usize;
                let child = if op == NEWCLOSURE {
                    proto.children.get(d).copied()
                } else {
                    proto
                        .closures
                        .iter()
                        .find(|(k, _)| *k == d)
                        .map(|&(_, f)| f)
                };
                let child = child.filter(|&child| child < protos.len());
                let child = child.ok_or("a closure of a function there's none of")?;
                let mut locals = Vec::new();
                for at in 0..protos[child].upvalues {
                    let capture = proto.code.get(pc + 1 + at).copied().unwrap_or(0);
                    if capture as u8 != CAPTURE {
                        return Err("a closure without its captures".to_owned());
                    }
                    let (kind, index) = ((capture >> 8) as u8, (capture >> 16) as u8 as usize);
                    let upvalue = (chunk_locals[parent].as_ref())
                        .and_then(|locals| locals.get(index).copied())
                        .unwrap_or(false);
                    locals.push(if kind == CAPTURE_UPVAL {
                        upvalue
                    } else {
                        parent == main
                    });
                }
                if chunk_locals[child].replace(locals).is_none() {
                    scan.push(child);
                }
            }
            pc += if WITH_AUX.contains(&op) { 2 } else { 1 };
        }
    }
    for (at, proto) in protos.iter().enumerate().filter(|&(at, _)| at != main) {
        let Some(locals) = &chunk_locals[at] else {
            continue;
        };
        let mut pc = 0;
        while let Some(&insn) = proto.code.get(pc) {
            let op = insn as u8;
            let upvalue = (insn >> 16) as u8 as usize;
            if op == SETUPVAL && locals.get(upvalue) == Some(&true) {
                let name = proto.names.get(upvalue).copied().unwrap_or(0) as usize;
                let local = name.checked_sub(1).and_then(|name| names.get(name));
                return Ok(Some(Assigned {
                    line: proto.line(pc),
                    local: local.cloned().unwrap_or_else(|| "(a local)".to_owned()),
                }));
            }
            pc += if WITH_AUX.contains(&op) { 2 } else { 1 };
        }
    }
    Ok(None)
}

/// Reads bytecode from where it's at, as the loader's `read` and `readVarInt` do.
struct Reader<'b> {
    bytes: &'b [u8],
    at: usize,
}

impl<'b> Reader<'b> {
    fn bytes(&mut self, count: usize) -> Result<&'b [u8], String> {
        let read = self.bytes.get(self.at..self.at.saturating_add(count));
        let read = read.ok_or("bytecode cut short")?;
        self.at += count;
        Ok(read)
    }

    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.bytes(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, String> {
        let bytes = self.bytes(4)?;
        Ok(u32::from_le_bytes(bytes.try_into().expect("4 bytes")))
    }

    fn var(&mut self) -> Result<u32, String> {
        Ok(self.var64()? as u32)
    }

    fn var64(&mut self) -> Result<u64, String> {
        let mut value = 0;
        for shift in (0..64).step_by(7) {
            let byte = self.byte()?;
            value |= u64::from(byte & 127) << shift;
            if byte & 128 == 0 {
                return Ok(value);
            }
        }
        Err("a varint of more than 64 bits".to_owned())
    }

    /// A function, as the loader's loop over the proto table reads one of `version`, whose type
    /// information is of `types`.
    fn proto(&mut self, version: u8, types: u8) -> Result<Proto, String> {
        let _stack = self.byte()?;
        let _params = self.byte()?;
        let upvalues = usize::from(self.byte()?);
        let _vararg = self.byte()?;
        let mut flags = 0;
        if version >= 4 {
            flags = self.byte()?;
            if types != 0 {
                let size = self.var()? as usize;
                self.bytes(size)?;
            }
        }
        let count = self.var()? as usize;
        let code = (0..count)
            .map(|_| self.u32())
            .collect::<Result<Vec<_>, _>>()?;
        let mut closures = Vec::new();
        for k in 0..self.var()? as usize {
            match self.byte()? {
                constant::NIL => {}
                constant::BOOLEAN => drop(self.bytes(1)?),
                constant::NUMBER => drop(self.bytes(8)?),
                constant::VECTOR => drop(self.bytes(16)?),
                constant::VECTORD => drop(self.bytes(32)?),
                constant::STRING => drop(self.var()?),
                constant::IMPORT => drop(self.u32()?),
                constant::TABLE => {
                    for _ in 0..self.var()? {
                        self.var()?;
                    }
                }
                constant::TABLE_WITH_CONSTANTS => {
                    for _ in 0..self.var()? {
                        self.var()?;
                        self.u32()?;
                    }
                }
                constant::CLOSURE => closures.push((k, self.var()? as usize)),
                constant::CLASS_SHAPE => {
                    self.var()?;
                    let members = u64::from(self.var()?) + u64::from(self.var()?);
                    for _ in 0..members {
                        self.var()?;
                    }
                }
                constant::INTEGER => {
                    self.byte()?;
                    self.var64()?;
                }
                kind => return Err(format!("a constant of kind {kind}, which there's none of")),
            }
        }
        let children = (0..self.var()?).map(|_| self.var().map(|child| child as usize));
        let children = children.collect::<Result<Vec<_>, _>>()?;
        let _defined = self.var()?;
        let _name = self.var()?;
        let mut lines = None;
        if self.byte()? != 0 {
            let gap = self.byte()?;
            let intervals = (count.saturating_sub(1) >> gap) + 1;
            let mut offset = 0u8;
            let offsets = (self.bytes(count)?.iter())
                .map(|&delta| {
                    offset = offset.wrapping_add(delta);
                    offset
                })
                .collect();
            let mut line = 0i32;
            let mut absolute = Vec::new();
            for _ in 0..intervals {
                line = line.wrapping_add(self.u32()? as i32);
                absolute.push(line);
            }
            lines = Some((gap, offsets, absolute));
        }
        let mut names = vec![0; upvalues];
        if self.byte()? != 0 {
            for _ in 0..self.var()? {
                self.var()?;
                self.var()?;
                self.var()?;
                self.byte()?;
            }
            let count = self.var()? as usize;
            names = (0..count).map(|_| self.var()).collect::<Result<_, _>>()?;
        }
        if version >= 11 {
            for _ in 0..self.var()? {
                self.byte()?;
                self.var()?;
            }
        }
        // The cost model's, of an inlinable function.
        if version >= 12 && flags & (1 << 3) != 0 {
            self.var64()?;
        }
        Ok(Proto {
            upvalues,
            code,
            closures,
            children,
            names,
            lines,
        })
    }
}
