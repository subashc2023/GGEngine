//! The harness gg-gpu's and gg-renderer's GPU tests share; hidden, as no game uses it.
//!
//! Each runs its tests in one process with a main of its own. It [`begin`]s before any thread
//! exists, then runs the tests one after another through [`Tests`]: those whose names contain a
//! non-flag argument, as libtest picks them, or all without one.

use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::Validation;

/// Readies a run of `name`'s GPU tests, logging into `dir`. Returns the JSON log's path, or `None`
/// where the tests are skipped, which it prints.
///
/// # Safety
///
/// No other thread may exist yet: it sets the environment.
///
/// # Panics
///
/// - With gg-core's `tracy` feature, which starts Tracy's thread before main.
/// - If the JSON log can't be made.
pub unsafe fn begin(name: &str, dir: &Path) -> Option<PathBuf> {
    if gg_core::profile::ENABLED {
        panic!(
            "the GPU tests set the environment before any other thread exists, and Tracy's start \
             before main; run them without the tracy feature"
        );
    }
    // CI runners without a GPU or a Vulkan driver, Windows' and macOS's, set this. Linux CI runs
    // the tests on lavapipe instead.
    if std::env::var_os("GG_SKIP_GPU_TESTS").is_some_and(|value| !value.is_empty()) {
        println!("GPU tests skipped: GG_SKIP_GPU_TESTS is set");
        return None;
    }
    // Overlays such as Steam's and OBS's load into every instance as implicit layers and can add
    // validation errors of their own. Apps keep implicit layers, since RenderDoc's capture layer
    // is one; the loader warns about each one left out here. Unless `RUST_LOG` is set, only
    // warnings and errors are logged, and of the loader's and other layers' messages only errors,
    // so a passing run's output is its test results, not what each instance, device and pipeline
    // logs as it's made.
    // SAFETY: the caller guarantees that no other thread exists yet to read the environment.
    unsafe {
        std::env::set_var("VK_LOADER_LAYERS_DISABLE", "~implicit~");
        if std::env::var_os("RUST_LOG").is_none() {
            std::env::set_var("RUST_LOG", "warn,gg_gpu::vulkan::general=error");
        }
    }
    Some(gg_core::log::init(name, dir, true).expect("the tests read the JSON log"))
}

/// Runs the tests the arguments pick, one after another, and counts how they go.
pub struct Tests {
    /// The arguments that aren't libtest's flags or their values: a test runs if its name
    /// contains one, or if there are none.
    filters: Vec<String>,
    passed: usize,
    failed: usize,
}

impl Tests {
    pub fn from_args() -> Self {
        let mut filters = Vec::new();
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            let takes_value = matches!(
                arg.as_str(),
                "--test-threads"
                    | "--skip"
                    | "--logfile"
                    | "--format"
                    | "--color"
                    | "--shuffle-seed"
                    | "-Z"
            );
            if takes_value {
                args.next();
            } else if !arg.starts_with('-') {
                filters.push(arg);
            }
        }
        Self {
            filters,
            passed: 0,
            failed: 0,
        }
    }

    /// Runs `test` if the filters pick it, and prints whether it passed. A failing test stops
    /// there and the log shows its panic, but later tests still run.
    pub fn run(&mut self, name: &str, test: impl FnOnce()) {
        let picked = self.filters.is_empty()
            || (self.filters.iter()).any(|filter| name.contains(filter.as_str()));
        if !picked {
            return;
        }
        if panic::catch_unwind(AssertUnwindSafe(test)).is_ok() {
            self.passed += 1;
            println!("test {name} ... ok");
        } else {
            self.failed += 1;
            println!("test {name} ... FAILED");
        }
    }

    /// Prints how many tests passed and failed, and fails the run if any failed.
    pub fn finish(self) -> ExitCode {
        let Self { passed, failed, .. } = self;
        println!("test result: {passed} passed; {failed} failed");
        if failed == 0 {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        }
    }
}

/// Asserts that `validation` is on, synchronization validation among it, as GPU tests run.
#[track_caller]
pub fn assert_validating(validation: &Validation) {
    assert!(
        validation.enabled(),
        "GPU tests run under validation; install the Vulkan SDK's validation layer"
    );
    assert!(
        validation.synchronization(),
        "GPU tests run under synchronization validation; install a newer Vulkan SDK, whose \
         validation layer takes VK_EXT_layer_settings"
    );
}

/// Checks that validation reported nothing, once what it validated has been dropped.
#[track_caller]
pub fn check(validation: &Validation) {
    assert_eq!(
        validation.messages(),
        0,
        "validation reported problems; the log above says what"
    );
}
