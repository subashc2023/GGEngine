//! Code patches, built by `dx tools hotpatch` and delivered by the tool API's `patch`. This module
//! finds the game's `main` (what a patch is built against) and applies patches, which needs unsafe.

use std::panic::{self, AssertUnwindSafe};
use std::sync::OnceLock;

use gg_core::{Error, Result};
use subsecond::JumpTable;
use tracing::info;

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
/// from then on (such as the stack's calls of layer methods) run the patch's code. Logs
/// `Code patched` with the library.
///
/// Fails with `api.patch`, changing nothing, if:
/// - the process refuses the table, per [`refusal`];
/// - the patch's library doesn't load;
/// - it isn't a patch of this build (one exporting no `main`; subsecond panics on it).
pub(crate) fn apply(table: JumpTable, ticks: u64) -> Result<()> {
    let _zone = gg_core::zone!();
    if let Some(refused) = refusal(aslr_reference(), &table) {
        return Err(refused);
    }
    let library = table.lib.clone();
    let apply = move || {
        // SAFETY: only a process that can read the session file's token can ask for a patch, and
        // only the game's user can: `dirs::write_private` writes the file mode 0600 on Unix, with
        // a DACL allowing its owner alone on Windows, before it holds the token. The API trusts
        // that process as it trusts the user, since a made-up table could corrupt memory.
        // `gg run` asks only with a patch `dx tools hotpatch` built against this process's
        // executable and the `main` address the session file gives, from edits inside function
        // bodies alone (but for items declared in them), so its code lays out each type as this
        // process does. It applies on the main thread between ticks, the only caller of `HotFn`,
        // so no call reads the jump table as it changes.
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
    info!(library = %library.display(), ticks, "Code patched");
    Ok(())
}

/// Why a process whose `main` is at `main` refuses patch `table` before its library loads, if it
/// does:
/// - `main` is 0, so it takes no patches ([`aslr_reference`]);
/// - the table's reference is past `main`: subsecond subtracts it to find how far the OS moved the
///   executable and panics on a negative result, so the patch isn't for this process.
fn refusal(main: u64, table: &JumpTable) -> Option<Error> {
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
            let refused = apply(table(&none, reference), 0)
                .err()
                .map(|error| error.code());
            assert_eq!(refused, Some("api.patch"), "{reference:#x}");
        }
        // SAFETY: the table is read at once, and only a patch, which no test applies, changes it.
        assert!(unsafe { subsecond::get_jump_table() }.is_none());
    }
}
