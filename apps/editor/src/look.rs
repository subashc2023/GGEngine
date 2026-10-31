//! The editor's side of a game's look: the view settings the game keeps, its viewpoint, an outline,
//! a grid and whether colliders' outlines show, which any client sets, part by part, the last to
//! set a part winning, as the tool API's `look` says. A part goes to the game only as the user
//! changes it, or as it follows one the editor took, as the grid follows the camera that drives the
//! view; and every answer is a read of the look: the parts another client set since, the editor
//! takes, but for those changed since it asked, which it sends next. It talks to no game itself:
//! the layer sends what it asks for, and hands it what the game answers.

use std::mem;

use gg_api::{By, Look, Looked, Outline, Plane, Viewpoint};

/// One of the look's parts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Viewpoint,
    Outline,
    Grid,
    Colliders,
}

/// The look's parts, each `None` where it's the game's own: what the editor wants of them, or knows
/// the game holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parts {
    pub viewpoint: Option<Viewpoint>,
    pub outline: Option<Outline>,
    pub grid: Option<Plane>,
    pub colliders: Option<bool>,
}

/// What the editor knows of a game's look, and what it asks of it: the look as the game last
/// answered, who set each part, and its revision; the latest revision a frame drew with; the parts
/// the user changed since the editor last asked; and the look it asked for, until the game answers.
pub struct Looking {
    /// The client the editor is to the game, as the game names a part's setter.
    client: &'static str,
    known: Parts,
    by: By,
    revision: u64,
    drawn: u64,
    /// Whether each part changed since the last look was asked for, by the user or as it follows
    /// one taken, by [`Part`].
    changed: [bool; 4],
    /// The parts the look asked for sets, all left out for a read.
    asked: Option<Look>,
}

impl Looking {
    /// For a game the editor is `client` to, as [`reset`](Self::reset) leaves it.
    pub fn new(client: &'static str) -> Self {
        Self {
            client,
            known: Parts::default(),
            by: By::default(),
            revision: 0,
            drawn: 0,
            changed: [true; 4],
            asked: None,
        }
    }

    /// Forgets the look and what was asked of it, for a game that serves anew, whose run starts
    /// with none, or as the game goes: so that the parts the editor wants that aren't the game's
    /// own go to the next to serve.
    pub fn reset(&mut self) {
        *self = Self::new(self.client);
    }

    /// Has the user's change of `part`, what the editor wants of it, go to the game, unless the
    /// look already holds what it wants, as the next [`ask`](Self::ask) finds it.
    pub fn change(&mut self, part: Part) {
        self.changed[part as usize] = true;
    }

    /// Has `part` go to the game as what the editor wants of it follows another part the editor
    /// took, as the grid follows the camera that drives the view, as [`change`](Self::change)
    /// does; but not if another client set it last, whose it stays.
    pub fn follow(&mut self, part: Part) {
        let by = match part {
            Part::Viewpoint => &self.by.viewpoint,
            Part::Outline => &self.by.outline,
            Part::Grid => &self.by.grid,
            Part::Colliders => &self.by.colliders,
        };
        if by.as_deref().is_none_or(|by| by == self.client) {
            self.change(part);
        }
    }

    /// Notes that a frame drew with the look's `revision`, which past what the editor knows has it
    /// read the look.
    pub fn drew(&mut self, revision: u64) {
        self.drawn = revision;
    }

    /// The look to send the game, while none waits for its answer: the parts changed since the
    /// last, of those `wanted` that the look doesn't hold, as they're wanted now; or else, if a
    /// frame drew with a later look than the editor knows, a read, which sets nothing.
    pub fn ask(&mut self, wanted: &Parts) -> Option<Look> {
        if self.asked.is_some() {
            return None;
        }
        let [viewpoint, outline, grid, colliders] = mem::take(&mut self.changed);
        let known = &self.known;
        let look = Look {
            viewpoint: sent(viewpoint, &wanted.viewpoint, &known.viewpoint),
            outline: sent(outline, &wanted.outline, &known.outline),
            grid: sent(grid, &wanted.grid, &known.grid),
            colliders: sent(colliders, &wanted.colliders, &known.colliders),
        };
        if look == Look::default() && self.drawn <= self.revision {
            return None;
        }
        self.asked = Some(look.clone());
        Some(look)
    }

    /// Takes `looked`, the game's answer to the look asked for, and returns what the editor takes of
    /// it, as a [`Look`] sets parts: each part another client set or gave back since the editor last
    /// knew it, but those the user changed since the look went out, which go next instead. The
    /// parts the editor set, in that look or before, are its own.
    pub fn read(&mut self, looked: Looked) -> Look {
        let asked = self.asked.take().unwrap_or_default();
        let Looked {
            viewpoint,
            outline,
            grid,
            colliders,
            by,
            revision,
            ..
        } = looked;
        let changed = self.changed;
        let kept = |part: Part, sets: bool| sets || changed[part as usize];
        let (known, setters, client) = (&mut self.known, &mut self.by, self.client);
        let taken = Look {
            viewpoint: take(
                (&mut known.viewpoint, &mut setters.viewpoint),
                (viewpoint, by.viewpoint),
                kept(Part::Viewpoint, asked.viewpoint.is_some()),
                client,
            ),
            outline: take(
                (&mut known.outline, &mut setters.outline),
                (outline, by.outline),
                kept(Part::Outline, asked.outline.is_some()),
                client,
            ),
            grid: take(
                (&mut known.grid, &mut setters.grid),
                (grid, by.grid),
                kept(Part::Grid, asked.grid.is_some()),
                client,
            ),
            colliders: take(
                (&mut known.colliders, &mut setters.colliders),
                (colliders, by.colliders),
                kept(Part::Colliders, asked.colliders.is_some()),
                client,
            ),
        };
        self.revision = revision;
        taken
    }

    /// Notes that the game refused the look asked for, or answered it with what isn't one: the
    /// look is as the editor knew it.
    pub fn failed(&mut self) {
        self.asked = None;
    }
}

/// The part to send, if the user `changed` it and the look doesn't hold what's `wanted` of it, as
/// the editor `known` it to: a value sets it, and `None` gives it back to the game.
fn sent<T: Clone + PartialEq>(
    changed: bool,
    wanted: &Option<T>,
    known: &Option<T>,
) -> Option<Option<T>> {
    (changed && wanted != known).then(|| wanted.clone())
}

/// What the editor takes of a part the game answered with, `value` and who set it, `by`, in place
/// of the part as it's `known`, and who set it: the part, if another client set it or gave it back
/// since, but not if it's `kept`, set by the look asked for or changed by the user since. A part
/// that `client`, the editor, set is its own.
fn take<T: Clone + PartialEq>(
    (known, setter): (&mut Option<T>, &mut Option<String>),
    (value, by): (Option<T>, Option<String>),
    kept: bool,
    client: &str,
) -> Option<Option<T>> {
    let theirs = by.as_deref() != Some(client) && (value != *known || by != *setter);
    let taken = (theirs && !kept).then(|| value.clone());
    (*known, *setter) = (value, by);
    taken
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIENT: &str = "editor";

    /// What the game answers with the look of `parts`, set as `by` says, at `revision`.
    fn looked(parts: &Parts, by: By, revision: u64) -> Looked {
        Looked {
            viewpoint: parts.viewpoint,
            outline: parts.outline.clone(),
            grid: parts.grid,
            colliders: parts.colliders,
            by,
            revision,
            run: 1,
            seen: None,
        }
    }

    /// Who set each part: `viewpoint`'s, the outline's, the grid's and the colliders' setters.
    fn by(setters: [Option<&str>; 4]) -> By {
        let [viewpoint, outline, grid, colliders] = setters.map(|by| by.map(str::to_owned));
        By {
            viewpoint,
            outline,
            grid,
            colliders,
        }
    }

    fn outline(id: &str) -> Outline {
        Outline {
            id: id.to_owned(),
            color: [1.0, 0.35, 0.02, 1.0],
            width: 3.0,
        }
    }

    /// A look the editor serves anew, its colliders' outlines set: they're wanted, and the game's.
    fn served(wanted: &Parts) -> Looking {
        let mut looking = Looking::new(CLIENT);
        let sent = looking.ask(wanted).expect("the parts not the game's own");
        let colliders = Look {
            colliders: Some(Some(true)),
            ..Look::default()
        };
        assert_eq!(
            sent, colliders,
            "the parts the editor wants that aren't the game's own"
        );
        assert_eq!(looking.ask(wanted), None, "one at a time");
        let mine = by([None, None, None, Some(CLIENT)]);
        let taken = looking.read(looked(wanted, mine, 1));
        assert_eq!(taken, Look::default(), "its own");
        looking
    }

    /// A part goes to the game only as the user changes it, and only while the look doesn't hold
    /// what's wanted of it, a look at a time, the latest wanted as the last is answered; while
    /// nothing goes, a frame drawn with a later look than the editor knows has it read the look.
    /// A game that serves anew gets the parts the editor wants that aren't the game's own again.
    #[test]
    fn a_part_goes_only_as_the_user_changes_it() {
        let mut wanted = Parts {
            colliders: Some(true),
            ..Parts::default()
        };
        let mut looking = served(&wanted);
        wanted.grid = Some(Plane::Xz);
        assert_eq!(looking.ask(&wanted), None, "unchanged by the user");
        looking.change(Part::Grid);
        let grid = looking.ask(&wanted).expect("changed");
        assert_eq!(grid.grid, Some(Some(Plane::Xz)));
        assert_eq!(
            (grid.viewpoint, grid.outline, grid.colliders),
            (None, None, None)
        );
        // Changed twice while it's asked, then answered: the latest goes.
        for plane in [Plane::Xy, Plane::Yz] {
            wanted.grid = Some(plane);
            looking.change(Part::Grid);
            assert_eq!(looking.ask(&wanted), None, "{plane:?} waits");
        }
        let mine = by([None, None, Some(CLIENT), Some(CLIENT)]);
        let answered = Parts {
            grid: Some(Plane::Xz),
            ..wanted.clone()
        };
        assert_eq!(
            looking.read(looked(&answered, mine.clone(), 2)),
            Look::default()
        );
        let latest = looking.ask(&wanted).expect("changed since");
        assert_eq!(latest.grid, Some(Some(Plane::Yz)));
        assert_eq!(looking.read(looked(&wanted, mine, 3)), Look::default());
        // Changed back to what the look holds, and given back to the game: null goes, as the
        // user's own.
        looking.change(Part::Colliders);
        assert_eq!(looking.ask(&wanted), None, "the look holds it");
        wanted.grid = None;
        looking.change(Part::Grid);
        let back = looking.ask(&wanted).expect("given back");
        assert_eq!(back.grid, Some(None));
        let answered = looked(&wanted, by([None, None, None, Some(CLIENT)]), 4);
        assert_eq!(
            looking.read(answered),
            Look::default(),
            "the editor's own null"
        );
        // A frame drawn with the look at the revision known reads nothing; one drawn later does.
        looking.drew(4);
        assert_eq!(looking.ask(&wanted), None);
        looking.drew(5);
        assert_eq!(looking.ask(&wanted), Some(Look::default()), "a read");
        assert_eq!(looking.ask(&wanted), None, "one at a time");
        looking.failed();
        assert_eq!(looking.ask(&wanted), Some(Look::default()), "read again");
        // Served anew: what it wants that isn't the game's own goes again.
        looking.reset();
        let again = looking.ask(&wanted).expect("served anew");
        assert_eq!(again.colliders, Some(Some(true)));
        assert_eq!(again.grid, None, "the game's own");
    }

    /// Every answer is a read: each part another client set or gave back since is taken, the
    /// viewpoint and the outline as the grid and the colliders; a part unchanged since isn't taken
    /// again, nor one the editor set, nor one the user changed since the look went out, which goes
    /// next instead.
    #[test]
    fn the_parts_others_set_are_taken() {
        let wanted = Parts {
            colliders: Some(true),
            ..Parts::default()
        };
        let mut looking = served(&wanted);
        let viewpoint = Viewpoint {
            position: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            projection: gg_api::Projection::Orthographic {
                height: 4.0,
                near: -1.0,
                far: 1.0,
            },
        };
        let theirs = Parts {
            viewpoint: Some(viewpoint),
            outline: Some(outline("0123456789abcdef")),
            grid: Some(Plane::Xy),
            colliders: Some(false),
        };
        let agent = by([Some("agent"); 4]);
        looking.drew(2);
        assert_eq!(looking.ask(&wanted), Some(Look::default()), "a read");
        let taken = looking.read(looked(&theirs, agent.clone(), 2));
        let expected = Look {
            viewpoint: Some(theirs.viewpoint),
            outline: Some(theirs.outline.clone()),
            grid: Some(theirs.grid),
            colliders: Some(theirs.colliders),
        };
        assert_eq!(taken, expected);
        // As the editor's wanted parts follow what it took, the agent's colliders are kept: the
        // editor wants none.
        let wanted = Parts {
            viewpoint: Some(viewpoint),
            outline: Some(outline("0123456789abcdef")),
            grid: Some(Plane::Xy),
            colliders: None,
        };
        assert_eq!(looking.ask(&wanted), None, "nothing the user changed");
        looking.drew(3);
        assert!(looking.ask(&wanted).is_some());
        let taken = looking.read(looked(&theirs, agent.clone(), 3));
        assert_eq!(taken, Look::default(), "unchanged since");
        // The agent gives back the grid and outlines another, while the user changes the grid.
        looking.drew(4);
        assert!(looking.ask(&wanted).is_some());
        looking.change(Part::Grid);
        let changed = Parts {
            outline: Some(outline("fedcba9876543210")),
            grid: None,
            ..theirs.clone()
        };
        let given_back = by([Some("agent"), Some("agent"), None, Some("agent")]);
        let taken = looking.read(looked(&changed, given_back, 4));
        let expected = Look {
            outline: Some(changed.outline.clone()),
            ..Look::default()
        };
        assert_eq!(taken, expected, "but the grid the user changed");
        let grid = looking.ask(&wanted).expect("the user's grid");
        assert_eq!(grid.grid, Some(Some(Plane::Xy)));
        // A part the editor set stays its own, whoever answers.
        let mine = Parts {
            grid: Some(Plane::Xy),
            ..changed
        };
        let setters = by([Some("agent"), Some("agent"), Some(CLIENT), Some("agent")]);
        assert_eq!(
            looking.read(looked(&mine, setters.clone(), 5)),
            Look::default()
        );
        looking.drew(6);
        assert!(looking.ask(&wanted).is_some());
        let read = looking.read(looked(&mine, setters, 6));
        assert_eq!(read, Look::default(), "its own grid");
    }
}
