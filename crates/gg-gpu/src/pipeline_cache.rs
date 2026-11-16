//! A pipeline cache's data as the engine saves it: the driver's, behind an engine header that says
//! which GPU and driver made it and shows whether it was cut short or changed.
//!
//! Drivers check at most their own header's GPU and needn't survive damaged data, so the engine's
//! header is checked before a driver sees anything. The driver's own header is never read: drivers
//! differ in what it holds, and one driver between releases. Lavapipe's says 32 bytes, in data of
//! 32 in Mesa 25.0 and of 20 from 26.3 (Mesa's ec071eb2). The layout follows Arseny Kapoulkine's
//! "Robust pipeline cache serialization" (zeux.io, 2019).

use ash::vk;

/// Which GPU and driver a pipeline cache's data is for; it loads only into the same device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    pub(crate) vendor: u32,
    pub(crate) device: u32,
    pub(crate) driver: u32,
    /// What the driver says its cache data is compatible with.
    pub(crate) uuid: [u8; vk::UUID_SIZE],
}

impl Identity {
    pub(crate) fn new(properties: &vk::PhysicalDeviceProperties) -> Self {
        Self {
            vendor: properties.vendor_id,
            device: properties.device_id,
            driver: properties.driver_version,
            uuid: properties.pipeline_cache_uuid,
        }
    }
}

const MAGIC: [u8; 4] = *b"GGPC";
/// The header's layout; data saved with another isn't read.
const FORMAT: u32 = 1;
/// The build's pointer width: a 32-bit and a 64-bit driver of one version can share a UUID.
const POINTER_WIDTH: u32 = usize::BITS;
/// In order, little-endian: magic number, format, vendor, device, driver version, UUID, pointer
/// width, and the length and hash of the driver's data.
const HEADER: usize = 4 + 4 + 4 + 4 + 4 + vk::UUID_SIZE + 4 + 8 + 8;

/// Why saved data doesn't load.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// None, as in a first run.
    Empty,
    /// Another GPU's, driver's or engine version's: sound, but not for this device.
    Foreign(&'static str),
    /// Cut short or changed after it was saved, or never the engine's.
    Damaged(&'static str),
}

/// `payload`, the driver's data for a device of `identity`, behind the engine's header.
pub(crate) fn seal(identity: &Identity, payload: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(HEADER + payload.len());
    data.extend_from_slice(&MAGIC);
    for word in [FORMAT, identity.vendor, identity.device, identity.driver] {
        data.extend_from_slice(&word.to_le_bytes());
    }
    data.extend_from_slice(&identity.uuid);
    data.extend_from_slice(&POINTER_WIDTH.to_le_bytes());
    data.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    data.extend_from_slice(&fnv1a(payload).to_le_bytes());
    data.extend_from_slice(payload);
    data
}

/// The driver's data in `data`, which [`seal`] made, if it's whole and for a device of `identity`.
pub(crate) fn open<'a>(identity: &Identity, data: &'a [u8]) -> Result<&'a [u8], Refusal> {
    if data.is_empty() {
        return Err(Refusal::Empty);
    }
    let Some((header, payload)) = data.split_at_checked(HEADER) else {
        return Err(Refusal::Damaged("it's shorter than its header"));
    };
    let word = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().expect("4 bytes"));
    let long = |at: usize| u64::from_le_bytes(header[at..at + 8].try_into().expect("8 bytes"));
    if header[..4] != MAGIC {
        return Err(Refusal::Damaged("it doesn't begin as the engine's does"));
    }
    if word(4) != FORMAT {
        return Err(Refusal::Foreign("another version of the engine saved it"));
    }
    if (word(8), word(12)) != (identity.vendor, identity.device) {
        return Err(Refusal::Foreign("it's another GPU's"));
    }
    if word(16) != identity.driver || header[20..36] != identity.uuid {
        return Err(Refusal::Foreign("another driver made it"));
    }
    if word(36) != POINTER_WIDTH {
        return Err(Refusal::Foreign("it's for another pointer width"));
    }
    if long(40) != payload.len() as u64 {
        return Err(Refusal::Damaged("its length isn't the one saved"));
    }
    if long(48) != fnv1a(payload) {
        return Err(Refusal::Damaged("its bytes changed after it was saved"));
    }
    Ok(payload)
}

/// 64-bit FNV-1a, which shows damage, not tampering: the file is the user's own.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GPU: Identity = Identity {
        vendor: 0x10de,
        device: 0x2684,
        driver: 0x9890_8000,
        uuid: [7; vk::UUID_SIZE],
    };

    #[test]
    fn sealed_data_opens_to_the_drivers_whole() {
        // Mesa 26.3's lavapipe's: its header says 32 bytes, and the engine reads none of it.
        let payload = [32, 0, 0, 0, 1, 0, 0, 0, 5, 0, 1, 0, 0, 0, 0, 0, 9, 9, 9, 9];
        let data = seal(&GPU, &payload);
        assert_eq!(data.len(), HEADER + payload.len());
        assert_eq!(open(&GPU, &data), Ok(&payload[..]));
        assert_eq!(open(&GPU, &seal(&GPU, &[])), Ok(&[][..]));
    }

    #[test]
    fn data_another_device_saved_or_that_changed_is_refused() {
        let data = seal(&GPU, &[1, 2, 3, 4, 5, 6, 7, 8]);
        let refusal = |identity: &Identity, data: &[u8]| open(identity, data).err();
        let foreign = |refusal: Option<Refusal>| matches!(refusal, Some(Refusal::Foreign(_)));
        let damaged = |refusal: Option<Refusal>| matches!(refusal, Some(Refusal::Damaged(_)));
        assert_eq!(refusal(&GPU, &[]), Some(Refusal::Empty));

        let other = |change: fn(&mut Identity)| {
            let mut identity = GPU;
            change(&mut identity);
            foreign(refusal(&identity, &data))
        };
        assert!(other(|gpu| gpu.vendor = 0x1002), "another vendor's");
        assert!(other(|gpu| gpu.device = 0x2685), "another device's");
        assert!(other(|gpu| gpu.driver += 1), "another driver's");
        assert!(other(|gpu| gpu.uuid[0] ^= 1), "another UUID's");

        let changed = |at: usize| {
            let mut changed = data.clone();
            changed[at] ^= 1;
            refusal(&GPU, &changed)
        };
        assert!(damaged(changed(0)), "the magic number");
        assert!(foreign(changed(4)), "the format");
        assert!(foreign(changed(36)), "the pointer width");
        assert!(damaged(changed(40)), "the length");
        assert!(damaged(changed(48)), "the hash");
        for at in HEADER..data.len() {
            assert!(damaged(changed(at)), "byte {at}");
        }
        let cut = |len: usize| damaged(refusal(&GPU, &data[..len]));
        assert!(cut(data.len() - 1), "the data cut short");
        assert!(cut(HEADER - 1), "the header cut short");
        let longer = [data.as_slice(), &[0]].concat();
        assert!(damaged(refusal(&GPU, &longer)), "longer");
    }
}
