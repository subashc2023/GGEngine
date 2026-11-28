//! Code patches, built by `dx tools hotpatch` and delivered by the tool API's `patch`. This module
//! finds the game's `main` (what a patch is built against) and applies patches, which needs unsafe.

use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Mutex, OnceLock, PoisonError};

use gg_core::{Error, Result};
use subsecond::JumpTable;
use tracing::info;

/// Where the last restart in place's patch put each function, by where the executable has it, both
/// as this process loaded them; empty before one. Its layers' code calls `HotFn` with the patch's
/// addresses, which later patches map too ([`compose`]). A restart in place drops what the one
/// before built, so only the last one's matter.
static RESTARTED: Mutex<Option<HashMap<u64, u64>>> = Mutex::new(None);

/// Where the game's `main` is in its process: what `dx tools hotpatch` builds a patch against. 0
/// where subsecond finds no exported `main` (cargo's Windows and Linux builds) or without debug
/// assertions (subsecond's calls never look a patch up). A cargo debug build for macOS exports
/// `main`, but only patches of a `dx build --fat-binary` build suit a process.
pub(crate) fn aslr_reference() -> u64 {
    // subsecond looks it up without a lock until found, so concurrent asks (as in the tests that
    // start the API) would race.
    static REFERENCE: OnceLock<u64> = OnceLock::new();
    *REFERENCE.get_or_init(|| {
        if cfg!(debug_assertions) {
            subsecond::aslr_reference() as u64
        } else {
            0
        }
    })
}

/// Applies the patch `table` names at simulation tick `ticks`. Calls through subsecond's `HotFn`
/// from then on (such as the stack's calls of layer methods) run the patch's code, from the
/// executable's code and from that of the last restart in place's patch ([`compose`]). `restarts`
/// for the patch of a restart in place, whose own addresses later patches then map. Logs
/// `Code patched` with the library.
///
/// Fails with `api.patch`, changing nothing, if:
/// - the process refuses the table, per [`refusal`];
/// - the patch's library doesn't load;
/// - it isn't a patch of this build (one exporting no `main`; subsecond panics on it).
pub(crate) fn apply(mut table: JumpTable, ticks: u64, restarts: bool) -> Result<()> {
    let _zone = gg_core::zone!();
    if let Some(refused) = refusal(aslr_reference(), &table) {
        return Err(refused);
    }
    let library = table.lib.clone();
    // What subsecond adds to each key, as the OS moved the executable: `refusal` keeps it positive.
    let slide = aslr_reference() - table.aslr_reference;
    let keys: Vec<u64> = (table.map.keys())
        .map(|key| key.wrapping_add(slide))
        .collect();
    let mut restarted = RESTARTED.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(restarted) = &*restarted {
        compose(&mut table, restarted, slide);
    }
    let apply = move || {
        // SAFETY: only a process that can read the session file's token can ask for a patch, and
        // only the game's user can: `dirs::write_private` writes the file mode 0600 on Unix, with
        // a DACL allowing its owner alone on Windows, before it holds the token. The API trusts
        // that process as it trusts the user, since a made-up table could corrupt memory.
        // `gg run` asks only with a patch `dx tools hotpatch` built against this process's
        // executable and the `main` address the session file gives. No value laid out by code
        // other than the patch's reaches its code: a patch whose types are laid out unlike the
        // code the process runs comes with a restart in place, which ends the layers and drops
        // all the run held before this applies, and builds anew with the patch's code
        // (`restart::Restart`). Any other patch carries edits inside function bodies alone (but
        // for items, and macros that may declare some, in them), so its code lays out each type as
        // the code the process runs does. It applies on the main thread between ticks, the only
        // caller of `HotFn`, so no call reads the jump table as it changes.
        unsafe { subsecond::apply_patch(table) }
    };
    // subsecond panics, after loading the library and before taking its table, on a library that
    // exports no `main` and on addresses that overflow when moved. gg-core's panic hook logs it.
    match panic::catch_unwind(AssertUnwindSafe(apply)) {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            let message = format!("can't load the patch's library {}", library.display());
            return Err(Error::new("api.patch", message).with_source(error));
        }
        Err(_) => {
            let message = format!(
                "{} isn't a patch of this build: subsecond panicked on it",
                library.display()
            );
            return Err(Error::new("api.patch", message));
        }
    }
    if restarts {
        // SAFETY: read on the main thread, the only one that applies patches, just after this one
        // committed its table, which subsecond leaks, so the reference stays valid.
        let table = unsafe { subsecond::get_jump_table() };
        let map = table.map(|table| &table.map);
        let placed = keys.iter().filter_map(|key| Some((*key, *map?.get(key)?)));
        *restarted = Some(placed.collect());
    }
    info!(library = %library.display(), ticks, restarts, "Code patched");
    Ok(())
}

/// Adds to `table`'s map, before subsecond moves it by `slide`, the addresses the last restart in
/// place's patch gave its functions (`restarted`, by where the executable has them), each mapped
/// where the patch puts the function. A key already in the map stays.
///
/// The layers that patch's code built call `HotFn` with its addresses, which only this finds: a
/// layer's methods' `call_it` keep their names as its fields change, since the symbol's hash
/// covers the generic arguments' paths, not their layouts.
fn compose(table: &mut JumpTable, restarted: &HashMap<u64, u64>, slide: u64) {
    let added: Vec<(u64, u64)> = (table.map.iter())
        .filter_map(|(key, value)| {
            let address = restarted.get(&key.wrapping_add(slide))?;
            Some((address.wrapping_sub(slide), *value))
        })
        .collect();
    for (key, value) in added {
        table.map.entry(key).or_insert(value);
    }
}

/// Why a process whose `main` is at `main` refuses patch `table` before its library loads, if it
/// does:
/// - `main` is 0, so it takes no patches ([`aslr_reference`]);
/// - the table's reference is past `main`: subsecond subtracts it to find how far the OS moved the
///   executable and panics on a negative result, so the patch isn't for this process.
pub(crate) fn refusal(main: u64, table: &JumpTable) -> Option<Error> {
    let message = if main == 0 {
        "this build can't take patches: subsecond finds no `main` that its executable exports, as \
         that of a debug build `dx build --fat-binary` made does"
            .to_owned()
    } else if main < table.aslr_reference {
        format!(
            "the patch isn't for this process: its table's reference, {:#x}, is past where this \
             process's `main` is, {main:#x}",
            table.aslr_reference
        )
    } else {
        return None;
    };
    Some(Error::new("api.patch", message))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(lib: &str, reference: u64) -> JumpTable {
        JumpTable {
            lib: std::env::temp_dir().join(lib),
            map: Default::default(),
            aslr_reference: reference,
            new_base_address: 0,
            ifunc_count: 0,
        }
    }

    /// A process refuses a patch if its `main` is 0 or before the table's reference, and takes one
    /// whose reference is at or before it, as dx's tables are.
    #[test]
    fn a_patch_is_refused_unless_its_table_fits_the_process() {
        let main = 0x7ff6_1234_5678;
        let refused = |main, reference| {
            let refused = refusal(main, &table("patch.dll", reference));
            refused.map(|error| (error.code(), error.to_string()))
        };
        let untaken = refused(0, 0).expect("refused");
        assert!(untaken.1.contains("can't take patches"), "{}", untaken.1);
        let elsewhere = refused(main, main + 1).expect("refused");
        assert!(
            elsewhere.1.contains("isn't for this process"),
            "{}",
            elsewhere.1
        );
        assert_eq!([untaken.0, elsewhere.0], ["api.patch"; 2]);
        assert_eq!(refused(main, main), None);
        assert_eq!(refused(main, 0x1_4000_1000), None);
    }

    /// A patch that can't apply (refused, or its library doesn't load) leaves no jump table, so
    /// calls run the original code.
    #[test]
    fn a_patch_that_cant_apply_leaves_the_code_as_it_was() {
        let none = format!("gg-engine-patch-{}-none.dll", std::process::id());
        for reference in [0, u64::MAX] {
            let refused = apply(table(&none, reference), 0, false)
                .err()
                .map(|error| error.code());
            assert_eq!(refused, Some("api.patch"), "{reference:#x}");
        }
        // SAFETY: the table is read at once, and only a patch, which no test applies, changes it.
        assert!(unsafe { subsecond::get_jump_table() }.is_none());
    }

    /// A patch maps, beside where the executable has its functions, where the last restart in
    /// place's patch put them, less the slide subsecond adds back; a key it has stays.
    #[test]
    fn a_patch_maps_where_the_last_restart_put_its_functions_too() {
        let slide = 0x1000;
        let mut patch = table("patch.dylib", 0);
        // Where the executable has two functions, before the slide, and where the patch puts them.
        patch.map.insert(0x10, 0x5010);
        patch.map.insert(0x20, 0x5020);
        // Where the last restart put the first, and a function this patch doesn't hold.
        let restarted = HashMap::from([(0x1010, 0x9_0000), (0x1030, 0x9_1000)]);
        let mut kept = table("patch.dylib", 0);
        kept.map.clone_from(&patch.map);
        compose(&mut patch, &restarted, slide);
        assert_eq!(patch.map.get(&(0x9_0000 - slide)), Some(&0x5010));
        assert_eq!(patch.map.len(), 3);
        kept.map.insert(0x9_0000 - slide, 1);
        compose(&mut kept, &restarted, slide);
        assert_eq!(kept.map.get(&(0x9_0000 - slide)), Some(&1));
    }
}
