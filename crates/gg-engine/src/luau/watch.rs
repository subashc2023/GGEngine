//! Development builds' watch of the layers' script files, which reloads one as it's saved,
//! through an edit, as scene files are.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use tracing::warn;

use crate::Context;
use crate::layer::Stack;

/// The least time between looks, as scene files' is.
const LOOK: Duration = Duration::from_millis(200);

#[derive(Default)]
pub(crate) struct Files {
    files: Vec<File>,
    looked: Option<Instant>,
}

struct File {
    /// Its path from the project's root, which its edits name it by.
    name: String,
    path: PathBuf,
    stamp: Option<(SystemTime, u64)>,
    /// Its text as last read.
    text: String,
}

impl Files {
    /// Watches the script at `path`, named `name`, whose text is `text`.
    pub(crate) fn watch(&mut self, name: String, path: &Path, text: String) {
        self.files.retain(|file| file.name != name);
        self.files.push(File {
            name,
            path: path.to_owned(),
            stamp: stamp(path),
            text,
        });
    }
}

fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    fs::metadata(path)
        .and_then(|meta| Ok((meta.modified()?, meta.len())))
        .ok()
}

/// Between ticks, as a frame starts, reloads the scripts saved since the last look, each through
/// an edit that a recording keeps (so a replay reloads it there) and a lockstep host offers for a
/// tick both peers apply it before ([`Stack::offer_script`]). A script that doesn't
/// [`compile`](super::compiles) warns and runs as it did. A lockstep joiner doesn't look: it runs
/// its host's scripts.
pub(crate) fn look(cx: &mut Context, stack: &mut Stack) {
    let now = Instant::now();
    let files = &mut cx.dev.scripts;
    if !stack.watches_files()
        || files.files.is_empty()
        || files.looked.is_some_and(|looked| now - looked < LOOK)
    {
        return;
    }
    files.looked = Some(now);
    let mut saves = Vec::new();
    for file in &mut files.files {
        let stamp = stamp(&file.path);
        if stamp.is_none() || stamp == file.stamp {
            continue;
        }
        // Being written, or held by its writer: a later look reads it, its stamp unchanged.
        let Ok(text) = fs::read_to_string(&file.path) else {
            continue;
        };
        file.stamp = stamp;
        if text == file.text {
            continue;
        }
        file.text.clone_from(&text);
        if let Err(error) = super::compiles(&file.name, &text) {
            warn!(
                code = error.code(),
                script = file.name,
                "{error:#}; the script runs as it did"
            );
            continue;
        }
        saves.push((file.name.clone(), text));
    }
    for (path, text) in saves {
        stack.offer_script(cx, path, text);
    }
}
