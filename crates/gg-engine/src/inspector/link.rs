//! The Inspector's link, which another layer picks and edits through as the panels do, and the
//! fields a link may set.

use std::cell::RefCell;
use std::rc::Rc;

use super::fields::AXES;
use super::{Edit, Placed};
use crate::EntityId;
use crate::reflect::{Ident, Shape, Type, Types, Value};

/// A link to an [`Inspector`], which [`Inspector::link`] gives, through which another layer picks
/// and edits as the Inspector's panels do, such as with a tool's handles in the game's view: its
/// sets and creates join the Inspector's commands, which its next tick makes and a recording keeps,
/// and its Edit menu undoes and redoes them as its own: a gesture of the link's sets of one field,
/// of one entity or several, from its first set to its seal, one edit, as the log merges a
/// gesture's sets of one field. A layer keeps it, cloned, on the main thread, as layers are.
///
/// The values it's given are those of the world the Inspector shows: a remote Inspector's, as it
/// reads a game's, hold an entity by its ID and a texture, a model or a font by its name or a path
/// to its file, each as a [`Value::String`], which the game reads by its own world.
///
/// [`Inspector`]: super::Inspector
/// [`Inspector::link`]: super::Inspector::link
#[derive(Clone)]
pub struct InspectorLink(pub(super) Rc<RefCell<Linked>>);

/// What an Inspector shares with its links.
#[derive(Default)]
pub(super) struct Linked {
    /// The pick asked for since the Inspector's last pass, which its next makes.
    pub(super) pick: Option<Picking>,
    /// The sets, creates and seals made since the Inspector's last pass, in order, each set the
    /// latest of its field since the seal before it, where that latest came.
    pub(super) pending: Vec<Linking>,
    /// Whether sets the Inspector has queued wait for its next tick.
    pub(super) queued: bool,
    /// Whether a remote Inspector, as of its last pass, has edits it hasn't sent its game, or one
    /// whose answer hasn't come: sets, operations, undos and redos alike.
    pub(super) unanswered: bool,
    /// The entity picked last, the active one, and every entity picked, in the order they were,
    /// the active one last, as of the Inspector's last pass or tick.
    pub(super) picked: Option<EntityId>,
    pub(super) picks: Vec<EntityId>,
    /// How many undos and redos the Inspector's passes have asked for.
    pub(super) stepped: u64,
    /// Where the active pick is, and those a drag of a tool's handles moves, as of a remote
    /// Inspector's last pass, as [`InspectorLink::along`] says.
    pub(super) placed: Option<Placed>,
    pub(super) along: Option<Vec<Placed>>,
    /// The game a remote Inspector is to show from its next pass, which `Some(None)` leaves.
    pub(super) reach: Option<Option<gg_api::Session>>,
    /// The entity the panels asked to frame in the view since a layer last took it.
    pub(super) frame: Option<EntityId>,
    /// Whether a layer has taken the panels' frames, so that they offer to frame.
    pub(super) frames: bool,
}

/// A pick asked for through a link.
pub(super) enum Picking {
    /// Picks these entities alone, in this order, the last the active one; none unpicks.
    Only(Vec<EntityId>),
    /// Adds this entity to what's picked, as the active one, or takes it out if it's picked.
    Toggle(EntityId),
}

/// A set, a create or a seal made through a link.
pub(super) enum Linking {
    /// Sets a field of the entity with this ID.
    Set(EntityId, Edit),
    /// Creates a root with these components, each a type's name and its whole value.
    Create(Vec<(Ident, Value)>),
    /// Ends the link's gesture.
    Seal,
}

impl InspectorLink {
    /// The entity the Inspector has picked last, the active one, which its Inspector panel shows,
    /// as of its last pass or tick, by its ID, which outlives it: one despawned since stays
    /// picked, for an undo to bring it back.
    pub fn picked(&self) -> Option<EntityId> {
        self.0.borrow().picked
    }

    /// Every entity the Inspector has picked, as of its last pass or tick, by their IDs, in the
    /// order they were picked, the active one, [`picked`](Self::picked)'s, last.
    pub fn picks(&self) -> Vec<EntityId> {
        self.0.borrow().picks.clone()
    }

    /// Has the Inspector pick the entity whose ID is `id` alone as its next pass starts, before
    /// its panels, whether or not they show, opening the rows above its own, as its Find field
    /// does; or unpick, for `None`, or for an ID no living entity has then. A remote Inspector,
    /// which can't know that of an entity its game has just made, picks any ID but one a read of
    /// the game has found gone, and shows it despawned if the read it makes then finds none. A pick
    /// asked for later in the frame takes its place. A world cleared, replaced or restored before
    /// then, as [`World::serial`](crate::World::serial) says, drops it.
    pub fn pick(&self, id: Option<EntityId>) {
        self.0.borrow_mut().pick = Some(Picking::Only(id.into_iter().collect()));
    }

    /// Has the Inspector pick the entities whose IDs are `ids` alone, in this order, the last the
    /// active one, as [`pick`](Self::pick) picks one: those of them no living entity has left out,
    /// and none, or none alive, unpicking.
    pub fn pick_all(&self, ids: &[EntityId]) {
        self.0.borrow_mut().pick = Some(Picking::Only(ids.to_vec()));
    }

    /// Has the Inspector add the entity whose ID is `id` to what it has picked, as the active one,
    /// as its next pass starts; or take it out, if it's picked, the one picked before it becoming
    /// the active one if it was; as a Ctrl+click of a row of the Entities panel does. An ID no
    /// living entity has changes nothing. It takes the place of a pick asked for before it in the
    /// frame, as [`pick`](Self::pick) does.
    pub fn toggle(&self, id: EntityId) {
        self.0.borrow_mut().pick = Some(Picking::Toggle(id));
    }

    /// Sets field `path` of `entity`'s `component` to `value` at the Inspector's next tick, as the
    /// panels' sets are made, `entity`'s whatever's picked by then: as part of the link's gesture,
    /// which [`seal`](Self::seal) ends, so that the Edit menu undoes a drag of the field, of one
    /// entity or of several, in one step. The gesture's first set ends an edit of the panels' going on, and their presses and
    /// releases don't end the link's, though a set, an operation, an undo or a redo of theirs does.
    /// Between two of the Inspector's passes the link keeps the latest value of each field alone,
    /// after the sets made since the field's last, so that a mouse that reports a thousand moves a
    /// second doesn't set a thousand, and a vector set whole and one of its components still set in
    /// the order they came. Until the tick, the Inspector panel draws the field as the set will
    /// make it, while `entity` is picked; a refusal shows and is logged as the panels' do, but one
    /// of a component `entity` hasn't, which the panel doesn't draw, shows above its components,
    /// as an operation's does.
    ///
    /// `path` is a field of the component's own, such as `translation`, which holds nothing the
    /// panels set at a path of its own, as a struct, a list or an array does; or a component of a
    /// vector that's one, such as `translation.x`: neither the component whole nor inside a field
    /// of another kind, whose sets the panel doesn't draw among those of the fields around them. A
    /// debug build panics on one: on its path as it's set, and on its field's type as the
    /// Inspector's next pass takes it.
    pub fn set(&self, entity: EntityId, component: &str, path: &str, value: Value) {
        debug_assert!(
            own_field(path),
            "a link sets a field of a component's own, or a component of a vector that's one; \
             got `{path}`"
        );
        let pending = &mut self.0.borrow_mut().pending;
        // The gesture's sets, since the last seal.
        let seal = pending
            .iter()
            .rposition(|linking| matches!(linking, Linking::Seal));
        let gesture = &mut pending[seal.map_or(0, |seal| seal + 1)..];
        let same = gesture.iter().position(|linking| {
            matches!(linking, Linking::Set(id, edit)
                if *id == entity && edit.component == component && edit.path == path)
        });
        match same {
            Some(i) => {
                gesture[i..].rotate_left(1);
                if let Some(Linking::Set(_, edit)) = gesture.last_mut() {
                    edit.value = value;
                }
            }
            None => pending.push(Linking::Set(
                entity,
                Edit {
                    component: component.to_owned(),
                    path: path.to_owned(),
                    value,
                },
            )),
        }
    }

    /// Creates a root with `components`, each a type's name and its whole value, at the Inspector's
    /// next tick, as the Entity menu's Create Empty does, after the link's sets and seals made
    /// before it: it ends the link's gesture, the Edit menu undoes it in one step, and the
    /// Inspector picks the entity it makes, unless the pick changes first. A refusal shows above
    /// the components, and is logged, as the menu's does.
    pub fn create(&self, components: Vec<(Ident, Value)>) {
        let pending = &mut self.0.borrow_mut().pending;
        pending.push(Linking::Create(components));
    }

    /// Has a remote Inspector, which [`Inspector::remote`](super::Inspector::remote) makes, show
    /// the world of the game `session` names from its next pass, connecting to it, as it serves; or
    /// none, for `None`, as the game goes. Its pick and its open rows stay, since entities' IDs
    /// outlive a game's restarts, and what it knew of the game before goes. Another Inspector
    /// ignores it.
    pub fn reach(&self, session: Option<gg_api::Session>) {
        self.0.borrow_mut().reach = Some(session);
    }

    /// Ends the link's gesture, as the drag that made its sets ends: its next set begins another
    /// edit.
    pub fn seal(&self) {
        let pending = &mut self.0.borrow_mut().pending;
        if !matches!(pending.last(), Some(Linking::Seal)) {
            pending.push(Linking::Seal);
        }
    }

    /// Whether a set made through the link waits for the Inspector's tick: from the set until a
    /// tick has made it, or has dropped it, refused or of a world cleared, replaced or restored
    /// since; or, for a remote Inspector, until a read of the game after the game's answer to it
    /// shows it, or the game refuses it. Until then, the value set is what the caller draws the
    /// field as.
    pub fn waiting(&self) -> bool {
        let linked = self.0.borrow();
        let set = |linking: &Linking| matches!(linking, Linking::Set(..));
        linked.queued || linked.pending.iter().any(set)
    }

    /// Whether nothing made through the link, or the panels, waits for the game, for a remote
    /// Inspector: no set or create made through the link since its last pass, and, as of that
    /// pass, no edit it hasn't sent its game, nor one whose answer hasn't come, sets, operations,
    /// undos and redos alike, while it's connected. So a request that another connection to the
    /// game sends from then on reaches the game after all of them, as a save of what they edited
    /// must. Unlike [`waiting`](Self::waiting), it doesn't wait for a read to show what the game
    /// has answered. The local Inspector, whose edits wait for its tick rather than for a game,
    /// has none unanswered: it says only whether a set or a create made through the link waits for
    /// its next pass.
    pub fn idle(&self) -> bool {
        let linked = self.0.borrow();
        let edit = |linking: &Linking| !matches!(linking, Linking::Seal);
        !linked.unanswered && !linked.pending.iter().any(edit)
    }

    /// Where the entity picked is, for a remote Inspector, as the game's last read of it gave it,
    /// as of the Inspector's last pass: only while that read is of the entity picked, so `None`
    /// from a change of pick until a read of the new one comes, never the place of the one before;
    /// and `None` for an entity without a transform, or whose read gave no place, or none that's
    /// finite. The local Inspector gives `None`: a layer reads the app's own world itself.
    pub fn placed(&self) -> Option<Placed> {
        let linked = self.0.borrow();
        linked
            .placed
            .filter(|placed| linked.picked == Some(placed.id))
    }

    /// Where the picked entities are that a drag of a tool's handles moves, turns or scales, for a
    /// remote Inspector, as of its last pass: each picked entity with a transform and no picked
    /// ancestor, whose drag would move it already, the active one among them if it has none; each
    /// as the game's last read of it gave it, and as the sets made through the link since, which
    /// that read doesn't show yet, will leave it, as [`placed`](Self::placed) has the active one.
    /// `None` until a read of each of them has come since it was picked, or while the active one
    /// isn't placed; and for the local Inspector, as `placed` gives none.
    pub fn along(&self) -> Option<Vec<Placed>> {
        self.placed()?;
        self.0.borrow().along.clone()
    }

    /// The entity the Entities panel asked to frame in the view since the last call, which this
    /// takes: by a double click on its row, or its row's Frame in View, each of which picks it
    /// alone, unless its row is among several picked; or by F while the panel has the keys, the
    /// active one, with the rest picked. A layer frames every entity picked, as of when the active
    /// one is placed. The panel offers Frame in View once a layer has called this, so that an app
    /// with no view of its own to frame in doesn't.
    pub fn framed(&self) -> Option<EntityId> {
        let linked = &mut *self.0.borrow_mut();
        linked.frames = true;
        linked.frame.take()
    }

    /// How many undos and redos the Inspector's passes have asked for, from its Edit menu or its
    /// keys, which goes up as one does: an edit going on ends with each, so a drag that sees it
    /// change ends too.
    pub fn stepped(&self) -> u64 {
        self.0.borrow().stepped
    }

    /// The sets and the seals made through the link since the Inspector's last pass, in order: a
    /// set as its entity's ID, component, path and value, and a seal as `None`; its creates are
    /// [`creates`](Self::creates)'. For tests.
    #[doc(hidden)]
    pub fn pending(&self) -> Vec<Option<(EntityId, String, String, Value)>> {
        let linked = self.0.borrow();
        let pending = linked.pending.iter().filter_map(|linking| match linking {
            Linking::Set(id, edit) => Some(Some((
                *id,
                edit.component.clone(),
                edit.path.clone(),
                edit.value.clone(),
            ))),
            Linking::Create(_) => None,
            Linking::Seal => Some(None),
        });
        pending.collect()
    }

    /// The creates made through the link since the Inspector's last pass, in order, each as its
    /// components. For tests.
    #[doc(hidden)]
    pub fn creates(&self) -> Vec<Vec<(Ident, Value)>> {
        let linked = self.0.borrow();
        let creates = linked.pending.iter().filter_map(|linking| match linking {
            Linking::Create(components) => Some(components.clone()),
            _ => None,
        });
        creates.collect()
    }
}

/// Whether `path` is a field of a component's own, or a component of a vector that's one, as far
/// as its text tells: a name, and at most an axis after it.
fn own_field(path: &str) -> bool {
    let (field, axis) = path.split_once('.').unwrap_or((path, "x"));
    let named = !field.is_empty() && !field.contains('[');
    named && AXES.iter().any(|&(name, ..)| name == axis)
}

/// Whether `edit`, a link's, sets what a link may, as `types` describe its component: a field of
/// the component's own that holds nothing the panels set at a path of its own, as a struct, a list
/// or an array does; or a component of a vector that's one.
pub(super) fn links_field(types: &Types, edit: &Edit) -> bool {
    let Some(index) = types.find(&edit.component) else {
        return false;
    };
    let Shape::Struct { fields, .. } = &types.info(index).shape else {
        return false;
    };
    let (name, axis) = match edit.path.split_once('.') {
        Some((name, axis)) => (name, Some(axis)),
        None => (edit.path.as_str(), None),
    };
    let Some(field) = fields.iter().find(|field| field.name == name) else {
        return false;
    };
    match (&field.ty, axis) {
        (Type::Vec2 | Type::Vec3 | Type::Vec4 | Type::DVec3, Some(axis)) => {
            AXES.iter().any(|&(named, ..)| named == axis)
        }
        (_, Some(_)) | (Type::List(_) | Type::Array(..), None) => false,
        (&Type::Named(index), None) => !matches!(types.info(index).shape, Shape::Struct { .. }),
        (_, None) => true,
    }
}
