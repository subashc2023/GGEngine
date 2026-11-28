//! Authors' edits passing through a playing copy to the world as edited ([`Stack::play`]), so
//! `stop` keeps them and a scene save writes them, while what the copy's ticks did goes. An edit
//! of the copy alone (the API's `play`) passes nothing, nor does undoing or redoing it.

use gg_core::Result;
use tracing::warn;

use super::Context;
#[cfg(doc)]
use super::Stack;
use crate::ecs::{Log, Operation};
use crate::edits::{Done, Edit};
use crate::scene_files;

/// An author's edit of a playing copy, as the copy's log was before it took it.
pub(crate) struct Through {
    edit: Edit,
    /// For an undo or redo, the operations of the edit the author's log in the copy would step;
    /// for an append, those of its last edit, which the append adds to.
    next: Option<Vec<Operation>>,
    /// How many edits the author's log in the copy had done: the place there of the edit an
    /// apply logs or a redo steps, and one past that of the edit an undo steps.
    done: usize,
    /// Whether the edit changes the copy alone.
    play: bool,
}

impl Through {
    /// `None` while no copy plays, and for an edit no author makes. `play` marks an edit of the
    /// copy alone.
    pub(crate) fn before(cx: &Context, edit: &Edit, play: bool) -> Option<Self> {
        cx.dev.edited.as_ref()?;
        let log = log_of(cx, edit.author());
        let next = match edit {
            Edit::Undo { .. } | Edit::Append { .. } => log.and_then(Log::undoable),
            Edit::Redo { .. } => log.and_then(Log::redoable),
            Edit::Apply { .. } | Edit::Amend { .. } => None,
            Edit::Script { .. } | Edit::Scene { .. } | Edit::Renew => return None,
        };
        Some(Self {
            edit: edit.clone(),
            next: next.map(<[Operation]>::to_vec),
            done: log.map_or(0, |log| log.edits().len()),
            play,
        })
    }

    /// Passes the edit through to the world as edited, as `done` says the copy took it, warning
    /// `play.not_kept` where that world refuses it:
    /// - an apply, append or amend the copy took, as its log applied it, with the IDs its creates
    ///   and duplicates drew;
    /// - an append the copy refused, which dropped the edit it added to, as an undo of that edit;
    /// - an undo or redo, stepped by the copy or refused and dropped, where the world as edited
    ///   would step the same edit the copy's log did. Their logs step alike only as far as edits
    ///   passed alike: the copy's starts afresh at `play`, and the world as edited may refuse what
    ///   the copy took.
    ///
    /// Nothing passes once the copy's ticks have changed scene, as the worlds' entities are then
    /// different scenes'. Nothing passes for an edit of the copy alone, or for an undo or redo of
    /// one, which the copy's log has at the place `Edited::played` keeps.
    pub(crate) fn pass(self, cx: &mut Context, done: &Result<Done>) {
        let took = done.is_ok();
        let undo = matches!(self.edit, Edit::Undo { .. });
        let changes = cx.world().scene_changes();
        let author = self.edit.author();
        let (now, held) = log_of(cx, author).map_or((0, 0), |log| {
            (log.edits().len(), log.edits().len() + log.undone().len())
        });
        let edited = cx.dev.edited.as_mut().expect("a copy plays");
        // Any edit but an amend ends its author's gesture in the copy, refused or not
        // (`Authors::edit`), so it ends in the world as edited too, passed or not.
        if !matches!(self.edit, Edit::Amend { .. }) {
            edited.authors.seal(author);
        }
        // An entry's place in its log stays as it's undone and redone. A new entry takes the
        // place of those undone, which it drops; a refused step may drop one.
        let played = &mut edited.played;
        let stepped = match self.edit {
            Edit::Undo { .. } => self.done.checked_sub(1),
            Edit::Redo { .. } => Some(self.done),
            _ => {
                if now > self.done {
                    played.retain(|(by, at)| by != author || *at < self.done);
                    if self.play {
                        played.push((author.to_owned(), self.done));
                    }
                }
                None
            }
        };
        let alone =
            stepped.is_some_and(|at| played.iter().any(|(by, on)| by == author && *on == at));
        played.retain(|(by, at)| by != author || *at < held);
        if self.play || alone {
            return;
        }
        if edited.world.scene_changes() != changes {
            if matches!(done, Ok(Done::Applied(_) | Done::Stepped(Some(_)))) {
                let author = self.edit.author();
                warn!(
                    code = "play.not_kept",
                    author,
                    "the playing copy has changed scene, so an edit of {author}'s doesn't pass \
                     through to the world as edited, whose entities are another scene's: a stop \
                     brings that world back without it, and a scene save leaves it out"
                );
            }
            return;
        }
        let edit = match self.edit {
            Edit::Apply { author, operations } if took && !operations.is_empty() => {
                let operations = last(cx, &author);
                Edit::Apply { author, operations }
            }
            Edit::Append { author, .. } if took => {
                let operations = last(cx, &author);
                match self.next {
                    // Its last edit grew by what it added.
                    Some(before) => {
                        let operations = operations[before.len().min(operations.len())..].to_vec();
                        Edit::Append { author, operations }
                    }
                    // With no edit to add to, it applied as an edit of its own.
                    None => Edit::Apply { author, operations },
                }
            }
            // Refused, it rolled back and dropped the edit it added to, unless that roll back was
            // refused too and the copy's log kept it.
            Edit::Append { author, .. } => {
                let Some(base) = self.next else { return };
                let kept = log_of(cx, &author).and_then(Log::undoable) == Some(&base[..]);
                if kept || !steps(cx, &author, true, &base) {
                    return;
                }
                Edit::Undo { author }
            }
            amend @ Edit::Amend { .. } if took => amend,
            Edit::Undo { author } | Edit::Redo { author } => {
                // Refused for a scene change, the copy's log keeps the edit; refused otherwise,
                // it drops it.
                let kept = done
                    .as_ref()
                    .is_err_and(|error| error.code() == "ops.scene_changed");
                let Some(next) = self.next.filter(|_| !kept) else {
                    return;
                };
                if !steps(cx, &author, undo, &next) {
                    if matches!(done, Ok(Done::Stepped(Some(_)))) {
                        let step = if undo { "undo" } else { "redo" };
                        warn!(
                            code = "play.not_kept",
                            author,
                            "the playing copy's {step} doesn't pass through to the world as \
                             edited, which would step another edit of {author}'s: a stop brings \
                             that world back as it is, and a scene save writes it so"
                        );
                    }
                    return;
                }
                if undo {
                    Edit::Undo { author }
                } else {
                    Edit::Redo { author }
                }
            }
            _ => return,
        };
        let author = edit.author().to_owned();
        let applied = |cx: &mut Context, edit| cx.apply_edit(edit);
        let refused = cx.in_edited(|cx| scene_files::client_edit(cx, edit, applied).err());
        if let Some(Some(error)) = refused {
            // Entities code spawned as the copy played, such as a round's, are the copy's alone.
            let why = if error.code() == "ecs.no_entity" {
                ", as the entity is the playing copy's alone, such as one its code spawned"
            } else {
                ""
            };
            warn!(
                code = "play.not_kept",
                author,
                refused = error.code(),
                "the playing copy took an edit of {author}'s that the world as edited refuses{why}: \
                 a stop brings that world back without it, and a scene save leaves it out. \
                 {error:#}"
            );
        }
    }
}

/// `author`'s log of the world in `cx`'s place.
fn log_of<'a>(cx: &'a Context, author: &str) -> Option<&'a Log> {
    let mut logs = cx.authors.logs(cx.world());
    logs.find(|(name, _)| *name == author).map(|(_, log)| log)
}

/// Whether `author`'s log in the world as edited would undo, or else redo, an edit of `operations`.
fn steps(cx: &Context, author: &str, undo: bool, operations: &[Operation]) -> bool {
    let edited = cx.dev.edited.as_ref().expect("a copy plays");
    let mut logs = edited.authors.logs(&edited.world);
    let log = logs.find(|(name, _)| *name == author).map(|(_, log)| log);
    log.and_then(if undo { Log::undoable } else { Log::redoable }) == Some(operations)
}

/// The operations of `author`'s last edit of the world in `cx`'s place, as its log applied them.
fn last(cx: &Context, author: &str) -> Vec<Operation> {
    let edit = log_of(cx, author).and_then(Log::undoable);
    edit.map(<[Operation]>::to_vec).unwrap_or_default()
}
