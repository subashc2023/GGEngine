//! A Windows executable's resources, in the `.res` file link.exe takes: the version info Explorer's
//! Details and Task Manager show, and the icon, a PNG, which Windows reads in an icon resource
//! since Vista. The layouts are Microsoft's documented ones: `RESOURCEHEADER` in "Resource File
//! Formats", `GRPICONDIR` and `VS_VERSIONINFO`.

use crate::Failure;

/// The resource types, as `RT_ICON`, `RT_GROUP_ICON` and `RT_VERSION` number them.
const ICON: u16 = 3;
const GROUP_ICON: u16 = 14;
const VERSION: u16 = 16;

/// Memory flags, moveable, pure and discardable, as rc.exe marks an icon group. Windows has ignored
/// them since its 32-bit versions.
const FLAGS: u16 = 0x1030;

/// The resources' language, U.S. English, and the version strings' code page, Unicode: rc.exe's
/// defaults.
const LANGUAGE: u16 = 0x0409;
const CODE_PAGE: u16 = 1200;

/// The types of a version info node's value: bytes, or text.
const BINARY: u16 = 0;
const TEXT: u16 = 1;

/// The empty resource a 32-bit resource file begins with, which tells it from a 16-bit one: no
/// data, a header of 32 bytes, and type and name number 0.
const EMPTY: [u8; 32] = {
    let mut empty = [0; 32];
    empty[4] = 32;
    (empty[8], empty[9], empty[12], empty[13]) = (0xff, 0xff, 0xff, 0xff);
    empty
};

/// A resource file of game `name`'s version info, its executable `<name>.exe`, and, with `icon`, a
/// PNG of 256 by 256 pixels as icon 1 in icon group 1, which Explorer shows as the first group.
///
/// Fails with `build.version` if `version`, before semver's pre-release or build, isn't three
/// numbers of at most 65,535 each, as the version info's fixed part holds them.
pub(super) fn resources(
    name: &str,
    version: &str,
    icon: Option<&[u8]>,
) -> Result<Vec<u8>, Failure> {
    let fixed = fixed(version).ok_or_else(|| {
        let message = format!(
            "the package's version, {version}, isn't three numbers of at most 65,535 each, as \
             Windows' version info holds them"
        );
        Failure::new("build.version", message)
    })?;
    let mut file = EMPTY.to_vec();
    if let Some(png) = icon {
        resource(&mut file, ICON, png);
        resource(&mut file, GROUP_ICON, &group(png.len()));
    }
    resource(&mut file, VERSION, &version_info(name, version, fixed));
    Ok(file)
}

/// Adds resource 1 of type `kind`, holding `data`, to `file`: its header, its data, and zeros to
/// the next multiple of 4 bytes, where the next header begins.
fn resource(file: &mut Vec<u8>, kind: u16, data: &[u8]) {
    let words = [data.len() as u32, 32];
    file.extend(words.iter().flat_map(|word| word.to_le_bytes()));
    // The type and the name, each a number, which 0xffff before it says.
    for half in [0xffff, kind, 0xffff, 1] {
        file.extend(half.to_le_bytes());
    }
    // The data's version, the flags, the language, and a version and characteristics of the
    // resource's own, which no tool reads.
    file.extend(0u32.to_le_bytes());
    file.extend(FLAGS.to_le_bytes());
    file.extend(LANGUAGE.to_le_bytes());
    file.extend([0; 8]);
    file.extend(data);
    pad(file);
}

/// The icon group naming icon 1: a PNG of `bytes` bytes, 256 by 256 (a size of 0), 32 bits a pixel.
fn group(bytes: usize) -> Vec<u8> {
    let mut group = Vec::new();
    // Reserved, a group of icons rather than cursors, and one of them.
    for half in [0u16, 1, 1] {
        group.extend(half.to_le_bytes());
    }
    // Its width and height, colors in a palette, and a reserved byte.
    group.extend([0, 0, 0, 0]);
    for half in [1u16, 32] {
        group.extend(half.to_le_bytes());
    }
    group.extend((bytes as u32).to_le_bytes());
    group.extend(1u16.to_le_bytes());
    group
}

/// Game `name`'s version info for `version`, whose numbers are `fixed`: Windows' fixed part, for an
/// application, and the strings Explorer's Details show, in [`LANGUAGE`].
fn version_info(name: &str, version: &str, [major, minor, patch]: [u16; 3]) -> Vec<u8> {
    let (high, low) = (
        (u32::from(major) << 16) | u32::from(minor),
        u32::from(patch) << 16,
    );
    // `VS_FIXEDFILEINFO`: its signature and structure's version, the file's and the product's
    // versions, the mask of the flags that follow and none of them, for Windows NT's 32-bit and
    // later APIs, an application, and no subtype or date.
    let fixed = [
        0xfeef_04bd,
        0x0001_0000,
        high,
        low,
        high,
        low,
        0x3f,
        0,
        0x0004_0004,
        1,
        0,
        0,
        0,
    ];
    let fixed: Vec<u8> = fixed
        .iter()
        .flat_map(|word: &u32| word.to_le_bytes())
        .collect();
    let exe = format!("{name}.exe");
    let strings = [
        ("FileDescription", name),
        ("FileVersion", version),
        ("OriginalFilename", &exe),
        ("ProductName", name),
        ("ProductVersion", version),
    ]
    .map(|(key, value)| {
        let value = utf16(value);
        node(key, TEXT, (value.len() / 2) as u16, &value, &[])
    });
    let table = node(
        &format!("{LANGUAGE:04X}{CODE_PAGE:04X}"),
        TEXT,
        0,
        &[],
        &strings,
    );
    let string_info = node("StringFileInfo", TEXT, 0, &[], &[table]);
    let translation = [LANGUAGE.to_le_bytes(), CODE_PAGE.to_le_bytes()].concat();
    let translations = node("Translation", BINARY, 4, &translation, &[]);
    let var_info = node("VarFileInfo", TEXT, 0, &[], &[translations]);
    let fixed_length = fixed.len() as u16;
    node(
        "VS_VERSION_INFO",
        BINARY,
        fixed_length,
        &fixed,
        &[string_info, var_info],
    )
}

/// A node of the version info's tree, as each of its structures is: its length, leaving out the
/// zeros after its last child; `value_length`, in bytes for binary, in characters for text, NUL
/// counted; `kind`; its key, UTF-16 ending in NUL; then its value and children, each at a multiple
/// of 4 bytes from the node's start, itself one.
fn node(key: &str, kind: u16, value_length: u16, value: &[u8], children: &[Vec<u8>]) -> Vec<u8> {
    let mut node = vec![0; 2];
    node.extend(value_length.to_le_bytes());
    node.extend(kind.to_le_bytes());
    node.extend(utf16(key));
    pad(&mut node);
    node.extend(value);
    for child in children {
        pad(&mut node);
        node.extend(child);
    }
    let length = node.len() as u16;
    node[..2].copy_from_slice(&length.to_le_bytes());
    node
}

/// `text` in UTF-16, little-endian, ending in NUL.
fn utf16(text: &str) -> Vec<u8> {
    let units = text.encode_utf16().chain([0]);
    units.flat_map(u16::to_le_bytes).collect()
}

/// Zeros after `bytes` to the next multiple of 4.
fn pad(bytes: &mut Vec<u8>) {
    bytes.resize(bytes.len().next_multiple_of(4), 0);
}

/// The three numbers of semver `version` before any pre-release or build; `None` unless each fits
/// the fixed part's 16 bits.
fn fixed(version: &str) -> Option<[u16; 3]> {
    let numbers = version.split(['-', '+']).next()?;
    let mut parts = numbers.split('.').map(|part| part.parse::<u16>().ok());
    let fixed = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(fixed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Resource<'a> {
        kind: u16,
        id: u16,
        language: u16,
        data: &'a [u8],
    }

    /// `file`'s resources, read header by header, each beginning at a multiple of 4 bytes.
    fn read(file: &[u8]) -> Vec<Resource<'_>> {
        let half = |at: usize| u16::from_le_bytes([file[at], file[at + 1]]);
        let word = |at: usize| u32::from_le_bytes(file[at..at + 4].try_into().unwrap());
        let mut resources = Vec::new();
        let mut at = 0;
        while at < file.len() {
            assert_eq!(at % 4, 0, "a header at {at}");
            let (size, header) = (word(at) as usize, word(at + 4) as usize);
            assert_eq!(header, 32, "a header of numbered type and name");
            assert_eq!((half(at + 8), half(at + 12)), (0xffff, 0xffff));
            let data = &file[at + header..at + header + size];
            resources.push(Resource {
                kind: half(at + 10),
                id: half(at + 14),
                language: half(at + 22),
                data,
            });
            at = (at + header + size).next_multiple_of(4);
        }
        assert_eq!(at, file.len());
        resources
    }

    /// A version info node, as [`node`] writes it.
    #[derive(Debug)]
    struct Node {
        key: String,
        kind: u16,
        value: Vec<u8>,
        children: Vec<Node>,
    }

    /// The node at the start of `bytes`, and its children, read as Windows reads them: the value's
    /// length in characters for text.
    fn parse(bytes: &[u8]) -> Node {
        let half = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
        let (length, value_length, kind) = (half(0) as usize, half(2) as usize, half(4));
        let mut at = 6;
        let mut key = Vec::new();
        while half(at) != 0 {
            key.push(half(at));
            at += 2;
        }
        at = (at + 2).next_multiple_of(4);
        let value_bytes = if kind == TEXT {
            2 * value_length
        } else {
            value_length
        };
        let value = bytes[at..at + value_bytes].to_vec();
        at += value_bytes;
        let mut children = Vec::new();
        while at.next_multiple_of(4) < length {
            at = at.next_multiple_of(4);
            let child_length = half(at) as usize;
            children.push(parse(&bytes[at..]));
            at += child_length;
        }
        assert_eq!(at, length, "{key:?}'s children end where its length says");
        Node {
            key: String::from_utf16(&key).unwrap(),
            kind,
            value,
            children,
        }
    }

    /// Text in UTF-16, as a node holds it, without its NUL.
    fn text(value: &[u8]) -> String {
        let units: Vec<u16> = (value.chunks(2))
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let (last, text) = units.split_last().expect("a NUL");
        assert_eq!(*last, 0, "text ends in NUL");
        String::from_utf16(text).unwrap()
    }

    #[test]
    fn the_resources_hold_the_icon_its_group_and_the_version_info() {
        let png = b"\x89PNG\r\n\x1a\n and the rest of a PNG";
        let file = resources("Foxball", "1.2.3-beta.1", Some(png)).expect("resources");
        let read = read(&file);
        // The empty resource that begins every 32-bit resource file.
        let mut empty = [0; 32];
        empty[4] = 32;
        empty[8..10].copy_from_slice(&[0xff, 0xff]);
        empty[12..14].copy_from_slice(&[0xff, 0xff]);
        assert_eq!(file[..32], empty);
        let found: Vec<(u16, u16, u16)> = (read.iter().skip(1))
            .map(|resource| (resource.kind, resource.id, resource.language))
            .collect();
        assert_eq!(
            found,
            [
                (ICON, 1, LANGUAGE),
                (GROUP_ICON, 1, LANGUAGE),
                (VERSION, 1, LANGUAGE)
            ]
        );
        assert_eq!(read[1].data, png);
        // One icon, 256 by 256 pixels of 32 bits, of the PNG's size, number 1.
        let mut group = vec![0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 1, 0, 32, 0];
        group.extend((png.len() as u32).to_le_bytes());
        group.extend([1, 0]);
        assert_eq!(read[2].data, group);

        let info = parse(read[3].data);
        assert_eq!((info.key.as_str(), info.kind), ("VS_VERSION_INFO", BINARY));
        let word = |at: usize| u32::from_le_bytes(info.value[at..at + 4].try_into().unwrap());
        assert_eq!(info.value.len(), 52);
        // The signature, the file's and the product's versions, an application on Windows NT.
        assert_eq!(word(0), 0xfeef_04bd);
        assert_eq!([word(8), word(12)], [0x0001_0002, 0x0003_0000]);
        assert_eq!([word(16), word(20)], [0x0001_0002, 0x0003_0000]);
        assert_eq!([word(32), word(36)], [0x0004_0004, 1]);
        let [strings, vars] = &info.children[..] else {
            panic!("StringFileInfo and VarFileInfo: {info:?}");
        };
        assert_eq!(strings.key, "StringFileInfo");
        let [table] = &strings.children[..] else {
            panic!("a string table: {strings:?}");
        };
        assert_eq!(table.key, "040904B0");
        let found: Vec<(&str, String)> = (table.children.iter())
            .map(|string| (string.key.as_str(), text(&string.value)))
            .collect();
        let expected = [
            ("FileDescription", "Foxball"),
            ("FileVersion", "1.2.3-beta.1"),
            ("OriginalFilename", "Foxball.exe"),
            ("ProductName", "Foxball"),
            ("ProductVersion", "1.2.3-beta.1"),
        ]
        .map(|(key, value)| (key, value.to_owned()));
        assert_eq!(found, expected);
        assert_eq!(vars.key, "VarFileInfo");
        let [translation] = &vars.children[..] else {
            panic!("a translation: {vars:?}");
        };
        assert_eq!(translation.key, "Translation");
        assert_eq!(translation.value, [0x09, 0x04, 0xb0, 0x04]);
    }

    #[test]
    fn without_an_icon_there_is_the_version_info_alone() {
        let file = resources("Shards", "0.1.0", None).expect("resources");
        let kinds: Vec<u16> = read(&file).iter().map(|resource| resource.kind).collect();
        assert_eq!(kinds, [0, VERSION]);
    }

    #[test]
    fn a_version_the_fixed_part_cant_hold_is_refused() {
        assert_eq!(fixed("0.1.0"), Some([0, 1, 0]));
        assert_eq!(fixed("65535.2.3+build.7"), Some([65535, 2, 3]));
        for version in ["65536.0.0", "1.2", "1.2.3.4", "1.x.3", ""] {
            let refused = resources("Shards", version, None).err();
            let code = refused.map(|failure| failure.code);
            assert_eq!(code.as_deref(), Some("build.version"), "{version}");
        }
    }
}
