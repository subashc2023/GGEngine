//! Patches of the game's code, which `dx tools hotpatch` builds for the game's process and the tool
//! API's `patch` brings: where the game's `main` is in its process, which a patch is built against,
//! and applying a patch, which only unsafe code does.

use std::panic::{self, AssertUnwindSafe};
use std::sync::OnceLock;

use gg_core::{Error, Result};
use subsecond::JumpTable;
use tracing::info;

/// Where the game's `main` is in its process, which `dx tools hotpatch` builds a patch for the
/// process against: 0 where subsecond finds no `main` that the executable exports, as in a build
/// that cargo made for Windows or Linux, or in a build without debug assertions, without which
/// subsecond's calls never look a patch up. A debug build that cargo made for macOS exports its
/// `main` all the same, though only patches of the build that `dx build --fat-binary` made suit a
/// process.
pub(crate) fn aslr_reference() -> u64 {
    // subsecond looks it up without a lock until it finds it, so asking on threads at once, as the
    // tests that start the API do, would race.
    static REFERENCE: OnceLock<u64> = OnceLock::new();
    *REFERENCE.get_or_init(|| {
        if cfg!(debug_assertions) {
            subsecond::aslr_reference() as u64
        } else {
            0
        }
    })
}

/// Applies the patch `table` names, the simulation having run `ticks` ticks, so that each call made
/// through subsecond's `HotFn` from then on, as the stack's calls of its layers' methods are, runs
/// the patch's code for what it patches; and logs `Code patched`, naming the patch's library.
///
/// Fails with `api.patch`, changing nothing, if the process refuses the table, as [`refusal`] says,
/// if the patch's library doesn't load, or if it isn't a patch of this build, as one that exports
/// no `main` isn't, which subsecond panics on.
pub(crate) fn apply(table: JumpTable, ticks: u64) -> Result<()> {
    let _zone = gg_core::zone!();
    if let Some(refused) = refusal(aslr_reference(), &table) {
        return Err(refused);
    }
    let library = table.lib.clone();
    let apply = move || {
        // SAFETY: only a process that can read the session file's token can ask for a patch, and
        // the API trusts it as it trusts the user, since a table made up to could corrupt memory.
        // `gg run` asks only with a patch that `dx tools hotpatch` built against this process's
        // executable and the address of `main` in it that the session file gives, from edits it
        // found inside function bodies alone, but for the items declared in them, so that its code
        // lays out each type as this process does. It applies on the main thread, between ticks,
        // the one thread that calls through `HotFn`, so no call reads the jump table as it changes.
        unsafe { subsecond::apply_patch(table) }
    };
    // subsecond panics, after loading the library and before taking its table, at a library that
    // exports no `main` and at addresses that overflow as it moves them; gg-core's panic hook logs
    // the panic as it happens.
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

/// Why a process whose `main` is at `main` refuses the patch `table` names, before its library
/// loads, if it does: it can't take patches, its `main` being 0, as [`aslr_reference`] says; or the
/// table's reference is past its `main`, which subsecond subtracts the reference from to find how
/// far the OS moved the executable, panicking where that's less than nothing, so the patch isn't
/// for this process.
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

    /// The table of a patch whose library is `lib`, in the temporary directory, and whose
    /// reference is `reference`.
    fn table(lib: &str, reference: u64) -> JumpTable {
        JumpTable {
            lib: std::env::temp_dir().join(lib),
            map: Default::default(),
            aslr_reference: reference,
            new_base_address: 0,
            ifunc_count: 0,
        }
    }

    /// A process refuses a patch if its `main` is 0, or before the table's reference, and takes one
    /// whose reference is at or before its `main`, as a table that dx built for it has.
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

    /// A patch that can't apply, whether this process refuses it or its library doesn't load,
    /// leaves the process without a jump table, so that its calls run its code as they did.
    #[test]
    fn a_patch_that_cant_apply_leaves_the_code_as_it_was() {
        let none = format!("gg-engine-patch-{}-none.dll", std::process::id());
        for reference in [0, u64::MAX] {
            let refused = apply(table(&none, reference), 0)
                .err()
                .map(|error| error.code());
            assert_eq!(refused, Some("api.patch"), "{reference:#x}");
        }
        // SAFETY: the table is read at once, and no test applies a patch, which alone changes it.
        assert!(unsafe { subsecond::get_jump_table() }.is_none());
    }
}
