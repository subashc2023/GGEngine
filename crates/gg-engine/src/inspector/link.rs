//! The Inspector's link: how another layer picks and edits as the panels do, and the fields it may
//! set.

use std::cell::RefCell;
use std::rc::Rc;

use super::fields::AXES;
use super::{Edit, Placed};
use crate::EntityId;
use crate::reflect::{Ident, Shape, Type, Types, Value};

/// A link to an [`Inspector`], from [`Inspector::link`]: another layer, such as a tool's handles
/// in the game's view, picks and edits through it as the panels do.
///
/// - Its sets and creates join the Inspector's commands: the next tick makes them and a recording
///   keeps them. The Edit menu undoes and redoes them.
/// - A gesture is the link's sets from the first to its [seal](Self::seal), of any fields and
///   entities. It is one edit.
///
/// A layer keeps it, cloned, on the main thread.
///
/// Values are those of the world the Inspector shows. A remote Inspector's hold an entity by ID,
/// and a texture, model or font by name or file path, each a [`Value::String`] the game reads.
///
/// [`Inspector`]: super::Inspector
/// [`Inspector::link`]: super::Inspector::link
#[derive(Clone)]
pub struct InspectorLink(pub(super) Rc<RefCell<Linked>>);

/// What an Inspector shares with its links.
#[derive(Default)]
pub(super) struct Linked {
    /// The pick asked for since the Inspector's last pass.
    pub(super) pick: Option<Picking>,
    /// The sets, creates and seals made since the last pass, in order. A set is the latest of its
    /// field since the seal before it, at the place that latest came.
    pub(super) pending: Vec<Linking>,
    /// Whether sets the Inspector has queued wait for its next tick.
    pub(super) queued: bool,
    /// Whether a remote Inspector, as of its last pass, has edits unsent or unanswered.
    pub(super) unanswered: bool,
    /// The active entity, and every entity picked in order, the active one last, as of the last
    /// pass or tick.
    pub(super) picked: Option<EntityId>,
    pub(super) picks: Vec<EntityId>,
    /// How many undos and redos the Inspector's passes have asked for.
    pub(super) stepped: u64,
    /// For [`InspectorLink::placed`] and [`InspectorLink::along`], as of a remote Inspector's last
    /// pass.
    pub(super) placed: Option<Placed>,
    pub(super) along: Option<Vec<Placed>>,
    /// The game a remote Inspector shows from its next pass; `Some(None)` leaves it.
    pub(super) reach: Option<Option<gg_api::Session>>,
    /// Why it shows no game, as it leaves it, if the caller said ([`InspectorLink::unreachable`]).
    pub(super) why: Option<String>,
    /// The entity the panels asked to frame since a layer last took it.
    pub(super) frame: Option<EntityId>,
    /// Whether a layer has taken the panels' frames, so they offer to frame.
    pub(super) frames: bool,
}

/// A pick asked for through a link.
pub(super) enum Picking {
    /// Picks these entities alone, in this order, the last the active one; none unpicks.
    Only(Vec<EntityId>),
    /// Adds this entity as the active one, or takes it out if it's picked.
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
    /// The active entity, which the Inspector panel shows, as of the last pass or tick.
    ///
    /// An entity despawned since stays picked, for an undo to bring it back.
    pub fn picked(&self) -> Option<EntityId> {
        self.0.borrow().picked
    }

    /// Every entity picked, in pick order, the active one ([`picked`](Self::picked)) last.
    pub fn picks(&self) -> Vec<EntityId> {
        self.0.borrow().picks.clone()
    }

    /// Has the Inspector pick `id` alone as its next pass starts, before its panels, shown or not,
    /// opening the rows above it like its Find field.
    ///
    /// - `None`, or an ID no living entity has then, unpicks.
    /// - A remote Inspector can't know of an entity its game just made: it picks any ID not found
    ///   gone by a read, and shows it despawned if its next read finds none.
    /// - A later pick in the frame replaces it.
    /// - A world cleared, replaced or restored before then drops it
    ///   ([`World::serial`](crate::World::serial)).
    pub fn pick(&self, id: Option<EntityId>) {
        self.0.borrow_mut().pick = Some(Picking::Only(id.into_iter().collect()));
    }

    /// Like [`pick`](Self::pick), for `ids` in order, the last the active one. IDs no living entity
    /// has are left out; none left unpicks.
    pub fn pick_all(&self, ids: &[EntityId]) {
        self.0.borrow_mut().pick = Some(Picking::Only(ids.to_vec()));
    }

    /// Has the Inspector add `id` as the active one as its next pass starts, or remove it if
    /// picked, like a Ctrl+click on an Entities row.
    ///
    /// Removing the active one makes the one picked before it active. An ID no living entity has
    /// changes nothing. Like [`pick`](Self::pick), it replaces an earlier pick in the frame.
    pub fn toggle(&self, id: EntityId) {
        self.0.borrow_mut().pick = Some(Picking::Toggle(id));
    }

    /// Sets field `path` of `entity`'s `component` to `value` at the Inspector's next tick, on
    /// whatever `entity` is picked by then.
    ///
    /// The set joins the link's gesture, which [`seal`](Self::seal) ends, so the Edit menu undoes
    /// a drag in one step.
    ///
    /// - The gesture's first set ends a panel edit going on. Panel presses and releases don't end
    ///   the link's gesture; panel sets, operations, undos and redos do.
    /// - Between two passes only the latest value of each field is kept, so a mouse reporting a
    ///   thousand moves a second doesn't set a thousand. A vector set whole and one of its
    ///   components keep their order.
    /// - Until the tick, the Inspector panel draws the field as the set will make it.
    /// - A refusal shows and is logged like the panels'; one for a component `entity` lacks shows
    ///   above its components.
    ///
    /// `path` is a field of the component's own, such as `translation`, holding nothing the panels
    /// set at a path of its own (a struct, list or array); or a component of such a vector, such as
    /// `translation.x`. Not the component whole, nor inside a field of another kind. A debug build
    /// panics on one: on its path as it's set, on its field's type as the next pass takes it.
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

    /// Creates a root with `components`, each a type's name and whole value, at the Inspector's
    /// next tick, after the link's earlier sets and seals, like the Entity menu's Create Empty.
    ///
    /// It ends the link's gesture and is one undo. The Inspector picks the new entity unless the
    /// pick changes first. A refusal shows above the components and is logged.
    pub fn create(&self, components: Vec<(Ident, Value)>) {
        let pending = &mut self.0.borrow_mut().pending;
        pending.push(Linking::Create(components));
    }

    /// Has a remote Inspector ([`Inspector::remote`](super::Inspector::remote)) show the world of
    /// `session`'s game from its next pass, connecting as it serves; `None` shows none.
    ///
    /// Its pick and open rows stay, since entity IDs outlive a game's restarts; what it knew of the
    /// game goes. Another Inspector ignores it.
    pub fn reach(&self, session: Option<gg_api::Session>) {
        let mut linked = self.0.borrow_mut();
        (linked.reach, linked.why) = (Some(session), None);
    }

    /// Like [`reach`](Self::reach) with no game, the panels saying `why` where its world would
    /// show, such as that no project is open, rather than that they wait for a game.
    pub fn unreachable(&self, why: &str) {
        let mut linked = self.0.borrow_mut();
        (linked.reach, linked.why) = (Some(None), Some(why.to_owned()));
    }

    /// Ends the link's gesture, as the end of a drag does: the next set begins another edit.
    pub fn seal(&self) {
        let pending = &mut self.0.borrow_mut().pending;
        if !matches!(pending.last(), Some(Linking::Seal)) {
            pending.push(Linking::Seal);
        }
    }

    /// Whether a set made through the link is still waiting: until a tick makes or drops it
    /// (refused, or of a world cleared, replaced or restored since). For a remote Inspector, until
    /// a read after the game's answer shows it, or the game refuses it.
    ///
    /// Until then, the caller draws the field as the value set.
    pub fn waiting(&self) -> bool {
        let linked = self.0.borrow();
        let set = |linking: &Linking| matches!(linking, Linking::Set(..));
        linked.queued || linked.pending.iter().any(set)
    }

    /// Whether nothing made through the link or the panels waits for the game.
    ///
    /// For a remote Inspector: no set or create made through the link since its last pass, and,
    /// as of that pass, no edit unsent or unanswered while connected. So a request another
    /// connection sends from then on reaches the game after all of them, as a save must. Unlike
    /// [`waiting`](Self::waiting), it doesn't wait for a read to show the answer.
    ///
    /// The local Inspector has nothing unanswered: only a set or create awaiting its next pass
    /// makes it not idle.
    pub fn idle(&self) -> bool {
        let linked = self.0.borrow();
        let edit = |linking: &Linking| !matches!(linking, Linking::Seal);
        !linked.unanswered && !linked.pending.iter().any(edit)
    }

    /// Where the picked entity is, for a remote Inspector, by the game's last read of it as of the
    /// last pass.
    ///
    /// `None` from a change of pick until a read of the new one comes (never the old one's place);
    /// for an entity with no transform, or whose read gave no finite place; and for the local
    /// Inspector, whose layer reads the app's own world.
    pub fn placed(&self) -> Option<Placed> {
        let linked = self.0.borrow();
        linked
            .placed
            .filter(|placed| linked.picked == Some(placed.id))
    }

    /// Where the picked entities are that a drag of a tool's handles moves, turns or scales, for a
    /// remote Inspector, as of its last pass: each picked entity with a transform and no picked
    /// ancestor.
    ///
    /// Each is as the game's last read gave it, plus the link's sets that read doesn't show yet,
    /// like [`placed`](Self::placed).
    ///
    /// `None` until a read of each has come since it was picked, while the active one isn't
    /// placed, and for the local Inspector.
    pub fn along(&self) -> Option<Vec<Placed>> {
        self.placed()?;
        self.0.borrow().along.clone()
    }

    /// The entity the Entities panel asked to frame in the view since the last call, which takes
    /// it. The panel asks by a double click on its row, its row's Frame in View (each picks it
    /// alone unless its row is among several picked), or F while it has the keys (the active one).
    ///
    /// A layer frames every entity picked, once the active one is placed. The panel offers Frame in
    /// View only once a layer has called this, so an app with no view doesn't.
    pub fn framed(&self) -> Option<EntityId> {
        let linked = &mut *self.0.borrow_mut();
        linked.frames = true;
        linked.frame.take()
    }

    /// How many undos and redos the Inspector's passes have asked for, by its Edit menu or keys.
    /// Each ends an edit going on, so a drag that sees this change ends too.
    pub fn stepped(&self) -> u64 {
        self.0.borrow().stepped
    }

    /// The sets (ID, component, path, value) and seals (`None`) since the last pass, in order.
    /// Creates are [`creates`](Self::creates)'. For tests.
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

    /// The creates since the last pass, in order, each as its components. For tests.
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

/// Whether `path`'s text is a field name and at most an axis.
fn own_field(path: &str) -> bool {
    let (field, axis) = path.split_once('.').unwrap_or((path, "x"));
    let named = !field.is_empty() && !field.contains('[');
    named && AXES.iter().any(|&(name, ..)| name == axis)
}

/// Whether `edit`, a link's, sets what a link may, as `types` describe its component: see
/// [`InspectorLink::set`]'s `path`.
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
