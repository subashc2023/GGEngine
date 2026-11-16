//! The editor's side of a game's look: the view settings the game keeps. They are the viewpoint, an
//! outline, a grid, colliders' outlines, what the view shows in place of its light, and the passes
//! it leaves out. Any client sets them part by part, the last to set a part winning, as the tool
//! API's `look` says.
//!
//! A part goes to the game only as the user changes it, or as it follows one the editor took, as
//! the grid follows the camera driving the view. Every answer is a read of the look. The editor
//! takes the parts another client set since, except those changed since it asked, which it sends
//! next. It talks to no game itself: the layer sends its asks and hands it the answers.

use std::mem;

use gg_api::{By, Look, Looked, Outline, Pass, Plane, Shown, Viewpoint};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Viewpoint,
    Outline,
    Grid,
    Colliders,
    Show,
    Skip,
}

/// The look's parts, each `None` where it's the game's own: what the editor wants, or knows the
/// game holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parts {
    pub viewpoint: Option<Viewpoint>,
    pub outline: Option<Outline>,
    pub grid: Option<Plane>,
    pub colliders: Option<bool>,
    pub show: Option<Shown>,
    /// In [`Pass::ALL`]'s order, each once, never empty, as the game answers it.
    pub skip: Option<Vec<Pass>>,
}

/// What the editor knows of a game's look, and asks of it.
pub struct Looking {
    /// The editor's client name, as the game names a part's setter.
    client: &'static str,
    known: Parts,
    by: By,
    revision: u64,
    drawn: u64,
    /// Per [`Part`], whether it changed since the last ask, by the user or following one taken.
    changed: [bool; 6],
    /// The look asked for, until answered. A read sets no part.
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
            changed: [true; 6],
            asked: None,
        }
    }

    /// Forgets the look and what was asked, for a game serving anew, whose run starts with none, or
    /// as the game goes. The parts the editor wants that aren't the game's own then go to the next
    /// to serve.
    pub fn reset(&mut self) {
        *self = Self::new(self.client);
    }

    /// Sends the user's change of `part` to the game, unless the look holds it by the next
    /// [`ask`](Self::ask).
    pub fn change(&mut self, part: Part) {
        self.changed[part as usize] = true;
    }

    /// Sends `part` as it follows another part the editor took, such as the grid following the
    /// camera driving the view, like [`change`](Self::change). Not if another client set it last:
    /// it stays theirs.
    pub fn follow(&mut self, part: Part) {
        let by = match part {
            Part::Viewpoint => &self.by.viewpoint,
            Part::Outline => &self.by.outline,
            Part::Grid => &self.by.grid,
            Part::Colliders => &self.by.colliders,
            Part::Show => &self.by.show,
            Part::Skip => &self.by.skip,
        };
        if by.as_deref().is_none_or(|by| by == self.client) {
            self.change(part);
        }
    }

    /// Notes that a frame drew with look `revision`. One past what the editor knows has it read the
    /// look.
    pub fn drew(&mut self, revision: u64) {
        self.drawn = revision;
    }

    /// Whether a frame drawn with look revision `drawn` drew through the game's own camera, as far
    /// as the editor knows. It did if the look last read, the frame's, has no viewpoint, and the
    /// editor hasn't changed the viewpoint since or asked to set it in a look awaiting its answer.
    /// A frame of a later look waits for the editor to read it.
    pub fn through_games_camera(&self, drawn: u64) -> bool {
        let asked = (self.asked.as_ref()).is_some_and(|look| look.viewpoint.is_some());
        drawn == self.revision
            && self.known.viewpoint.is_none()
            && !self.changed[Part::Viewpoint as usize]
            && !asked
    }

    /// The look to send, while none awaits its answer: the parts changed since the last that the
    /// look doesn't hold as `wanted`, as wanted now. Else, if a frame drew with a later look than
    /// the editor knows, a read, which sets nothing.
    pub fn ask(&mut self, wanted: &Parts) -> Option<Look> {
        if self.asked.is_some() {
            return None;
        }
        let [viewpoint, outline, grid, colliders, show, skip] = mem::take(&mut self.changed);
        let known = &self.known;
        let look = Look {
            viewpoint: sent(viewpoint, &wanted.viewpoint, &known.viewpoint),
            outline: sent(outline, &wanted.outline, &known.outline),
            grid: sent(grid, &wanted.grid, &known.grid),
            colliders: sent(colliders, &wanted.colliders, &known.colliders),
            show: sent(show, &wanted.show, &known.show),
            skip: sent(skip, &wanted.skip, &known.skip),
        };
        if look == Look::default() && self.drawn <= self.revision {
            return None;
        }
        self.asked = Some(look.clone());
        Some(look)
    }

    /// Takes `looked`, the game's answer, and returns what the editor takes of it, as a [`Look`]
    /// sets parts: each part another client set or gave back since the editor knew it. The parts
    /// the user changed since the look went out go next instead. The parts the editor set, in that
    /// look or before, are its own.
    pub fn read(&mut self, looked: Looked) -> Look {
        let asked = self.asked.take().unwrap_or_default();
        let Looked {
            viewpoint,
            outline,
            grid,
            colliders,
            show,
            skip,
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
            show: take(
                (&mut known.show, &mut setters.show),
                (show, by.show),
                kept(Part::Show, asked.show.is_some()),
                client,
            ),
            skip: take(
                (&mut known.skip, &mut setters.skip),
                (skip, by.skip),
                kept(Part::Skip, asked.skip.is_some()),
                client,
            ),
        };
        self.revision = revision;
        taken
    }

    /// Notes that the game refused the look, or answered with what isn't one: the look stays as the
    /// editor knew it.
    pub fn failed(&mut self) {
        self.asked = None;
    }
}

/// The part to send, if the user `changed` it and the `known` look doesn't hold what's `wanted`: a
/// value sets it, `None` gives it back to the game.
fn sent<T: Clone + PartialEq>(
    changed: bool,
    wanted: &Option<T>,
    known: &Option<T>,
) -> Option<Option<T>> {
    (changed && wanted != known).then(|| wanted.clone())
}

/// What the editor takes of a part the game answered, `value` set `by`, in place of the `known`
/// one. It takes the part if another client set it or gave it back since, but not if `kept`: set by
/// the look asked for, or changed by the user since. A part `client`, the editor, set is its own.
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

    /// The game's answer with the look of `parts`, set as `by` says, at `revision`.
    fn looked(parts: &Parts, by: By, revision: u64) -> Looked {
        Looked {
            viewpoint: parts.viewpoint,
            outline: parts.outline.clone(),
            grid: parts.grid,
            colliders: parts.colliders,
            show: parts.show.clone(),
            skip: parts.skip.clone(),
            by,
            revision,
            run: 1,
            seen: None,
        }
    }

    /// Who set `viewpoint`, the outline, the grid and the colliders. None set what the view shows
    /// or the passes it leaves out.
    fn by(setters: [Option<&str>; 4]) -> By {
        let [viewpoint, outline, grid, colliders] = setters.map(|by| by.map(str::to_owned));
        By {
            viewpoint,
            outline,
            grid,
            colliders,
            show: None,
            skip: None,
        }
    }

    fn outline(id: &str) -> Outline {
        Outline {
            ids: vec![id.to_owned()],
            color: [1.0, 0.35, 0.02, 1.0],
            width: 3.0,
        }
    }

    /// A look the editor serves anew, its colliders' outlines set: wanted, and the game's.
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

    /// A part goes only as the user changes it, and only while the look doesn't hold it; a look at
    /// a time, the latest wanted as the last is answered. While nothing goes, a frame drawn with a
    /// later look than known has it read. A game serving anew again gets the parts wanted that
    /// aren't its own.
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
    /// viewpoint and the outline as the grid and the colliders. Not taken: a part unchanged since,
    /// one the editor set, or one the user changed since the look went out, which goes next.
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
            show: None,
            skip: None,
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
            show: None,
            skip: None,
        };
        assert_eq!(taken, expected);
        // As the editor's wanted parts follow what it took, the agent's colliders are kept: the
        // editor wants none.
        let wanted = Parts {
            viewpoint: Some(viewpoint),
            outline: Some(outline("0123456789abcdef")),
            grid: Some(Plane::Xy),
            colliders: None,
            show: None,
            skip: None,
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

    /// What the view shows goes as the other parts do: once as the user picks it, null as they give
    /// it back. An agent's is taken, null too, but not over the user's pick since, which goes next.
    #[test]
    fn what_the_view_shows_goes_and_is_taken_as_the_others_parts() {
        let mut wanted = Parts {
            colliders: Some(true),
            ..Parts::default()
        };
        let mut looking = served(&wanted);
        let setters = |show: Option<&str>| By {
            show: show.map(str::to_owned),
            ..by([None, None, None, Some(CLIENT)])
        };
        let others = wanted.clone();
        let shows = |show: Option<Shown>| Parts {
            show,
            ..others.clone()
        };
        // The user picks one: it goes once, and the answer, its own, takes nothing.
        wanted.show = Some(Shown::Normal);
        looking.change(Part::Show);
        let sent = looking.ask(&wanted).expect("picked");
        let expected = Look {
            show: Some(Some(Shown::Normal)),
            ..Look::default()
        };
        assert_eq!(sent, expected);
        assert_eq!(looking.ask(&wanted), None, "one at a time");
        let answer = looked(&wanted, setters(Some(CLIENT)), 2);
        assert_eq!(looking.read(answer), Look::default(), "its own");
        assert_eq!(looking.ask(&wanted), None, "sent once");
        // An agent sets another, which the editor takes.
        looking.drew(3);
        assert_eq!(looking.ask(&wanted), Some(Look::default()), "a read");
        let theirs = shows(Some(Shown::Roughness));
        let taken = looking.read(looked(&theirs, setters(Some("agent")), 3));
        let expected = Look {
            show: Some(Some(Shown::Roughness)),
            ..Look::default()
        };
        assert_eq!(taken, expected);
        wanted.show = Some(Shown::Roughness);
        assert_eq!(looking.ask(&wanted), None, "taken, not the user's change");
        // The user picks one as an agent sets an image: the user's goes next.
        looking.drew(4);
        assert!(looking.ask(&wanted).is_some());
        wanted.show = Some(Shown::Metallic);
        looking.change(Part::Show);
        let image = shows(Some(Shown::Image("view depth".to_owned())));
        let taken = looking.read(looked(&image, setters(Some("agent")), 4));
        assert_eq!(taken, Look::default(), "but the user's");
        let sent = looking.ask(&wanted).expect("the user's");
        assert_eq!(sent.show, Some(Some(Shown::Metallic)));
        let answer = looked(&wanted, setters(Some(CLIENT)), 5);
        assert_eq!(looking.read(answer), Look::default());
        // Given back: null goes.
        wanted.show = None;
        looking.change(Part::Show);
        let back = looking.ask(&wanted).expect("given back");
        assert_eq!(back.show, Some(None));
        let answer = looked(&wanted, setters(Some(CLIENT)), 6);
        assert_eq!(looking.read(answer), Look::default(), "the editor's null");
        // An agent sets an image, then gives it back: both are taken.
        looking.drew(7);
        assert!(looking.ask(&wanted).is_some());
        let theirs = shows(Some(Shown::Image("view ids".to_owned())));
        let taken = looking.read(looked(&theirs, setters(Some("agent")), 7));
        assert_eq!(taken.show, Some(theirs.show));
        looking.drew(8);
        assert!(looking.ask(&wanted).is_some());
        let taken = looking.read(looked(&wanted, setters(None), 8));
        assert_eq!(taken.show, Some(None), "the agent's null");
    }

    /// The passes left out go as the other parts do: once as the user changes them, null as every
    /// pass draws again. An agent's are taken, and its null too.
    #[test]
    fn the_passes_left_out_go_and_are_taken_as_the_others_parts() {
        let mut wanted = Parts {
            colliders: Some(true),
            ..Parts::default()
        };
        let mut looking = served(&wanted);
        let setters = |skip: Option<&str>| By {
            skip: skip.map(str::to_owned),
            ..by([None, None, None, Some(CLIENT)])
        };
        let skips = |skip: Option<Vec<Pass>>| Look {
            skip: Some(skip),
            ..Look::default()
        };
        // The user leaves reflections out: it goes once, and the answer, its own, takes nothing.
        wanted.skip = Some(vec![Pass::Reflections]);
        looking.change(Part::Skip);
        let sent = looking.ask(&wanted).expect("changed");
        assert_eq!(sent, skips(wanted.skip.clone()));
        let answer = looked(&wanted, setters(Some(CLIENT)), 2);
        assert_eq!(looking.read(answer), Look::default(), "its own");
        assert_eq!(looking.ask(&wanted), None, "sent once");
        // Drawn whole again: null goes.
        wanted.skip = None;
        looking.change(Part::Skip);
        assert_eq!(looking.ask(&wanted), Some(skips(None)));
        assert_eq!(
            looking.read(looked(&wanted, setters(None), 3)),
            Look::default()
        );
        // An agent leaves out others, which are taken, then gives them back, which is taken too.
        looking.drew(4);
        assert_eq!(looking.ask(&wanted), Some(Look::default()), "a read");
        let theirs = Parts {
            skip: Some(vec![Pass::Shadows, Pass::Fog]),
            ..wanted.clone()
        };
        let taken = looking.read(looked(&theirs, setters(Some("agent")), 4));
        assert_eq!(taken, skips(theirs.skip.clone()));
        wanted.skip = theirs.skip;
        looking.drew(5);
        assert!(looking.ask(&wanted).is_some());
        wanted.skip = None;
        let taken = looking.read(looked(&wanted, setters(None), 5));
        assert_eq!(taken, skips(None), "the agent's null");
    }
}
