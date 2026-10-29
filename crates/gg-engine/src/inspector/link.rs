//! The Inspector's link, which another layer picks and edits through as the panels do, and the
//! fields a link may set.

use std::cell::RefCell;
use std::rc::Rc;

use super::Edit;
use super::fields::AXES;
use crate::reflect::{Shape, Type, Value};
use crate::{EntityId, World};

/// A link to an [`Inspector`], which [`Inspector::link`] gives, through which another layer picks
/// and edits as the Inspector's panels do, such as with a tool's handles in the game's view: its
/// sets join the Inspector's commands, which its next tick makes and a recording keeps, and its
/// Edit menu undoes and redoes them as its own: a gesture of the link's sets of one field, from its
/// first set to its seal, one edit, as the log merges a gesture's sets of one field. A layer keeps
/// it, cloned, on the main thread, as layers are.
///
/// [`Inspector`]: super::Inspector
/// [`Inspector::link`]: super::Inspector::link
#[derive(Clone)]
pub struct InspectorLink(pub(super) Rc<RefCell<Linked>>);

/// What an Inspector shares with its links.
#[derive(Default)]
pub(super) struct Linked {
    /// The pick asked for since the Inspector's last pass, which its next makes: `Some(None)`
    /// unpicks.
    pub(super) pick: Option<Option<EntityId>>,
    /// The sets and the seals made since the Inspector's last pass, in order, each set the latest
    /// of its field since the seal before it, where that latest came.
    pub(super) pending: Vec<Linking>,
    /// Whether sets the Inspector has queued wait for its next tick.
    pub(super) queued: bool,
    /// The entity picked, as of the Inspector's last pass or tick.
    pub(super) picked: Option<EntityId>,
    /// How many undos and redos the Inspector's passes have asked for.
    pub(super) stepped: u64,
}

/// A set or a seal made through a link.
pub(super) enum Linking {
    /// Sets a field of the entity with this ID.
    Set(EntityId, Edit),
    /// Ends the link's gesture.
    Seal,
}

impl InspectorLink {
    /// The entity the Inspector has picked, as of its last pass or tick, by its ID, which outlives
    /// it: one despawned since stays picked, for an undo to bring it back.
    pub fn picked(&self) -> Option<EntityId> {
        self.0.borrow().picked
    }

    /// Has the Inspector pick the entity whose ID is `id` as its next pass starts, before its
    /// panels, whether or not they show, opening the rows above its own, as its Find field does;
    /// or unpick, for `None`, or for an ID no living entity has then. A pick asked for later in the
    /// frame takes its place. A world cleared, replaced or restored before then, as
    /// [`World::serial`] says, drops it.
    pub fn pick(&self, id: Option<EntityId>) {
        self.0.borrow_mut().pick = Some(id);
    }

    /// Sets field `path` of `entity`'s `component` to `value` at the Inspector's next tick, as the
    /// panels' sets are made, `entity`'s whatever's picked by then: as part of the link's gesture,
    /// which [`seal`](Self::seal) ends, so that the Edit menu undoes a drag of the field in one
    /// step. The gesture's first set ends an edit of the panels' going on, and their presses and
    /// releases don't end the link's, though a set, an operation, an undo or a redo of theirs does.
    /// Between two of the Inspector's passes the link keeps the latest value of each field alone,
    /// after the sets made since the field's last, so that a mouse that reports a thousand moves a
    /// second doesn't set a thousand, and a vector set whole and one of its components still set in
    /// the order they came. Until the tick, the Inspector panel draws the field as the set will
    /// make it, while `entity` is picked; a refusal shows and is logged as the panels' do.
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
    /// since. Until then, the value set is what the caller draws the field as.
    pub fn waiting(&self) -> bool {
        let linked = self.0.borrow();
        let set = |linking: &Linking| matches!(linking, Linking::Set(..));
        linked.queued || linked.pending.iter().any(set)
    }

    /// How many undos and redos the Inspector's passes have asked for, from its Edit menu or its
    /// keys, which goes up as one does: an edit going on ends with each, so a drag that sees it
    /// change ends too.
    pub fn stepped(&self) -> u64 {
        self.0.borrow().stepped
    }

    /// The sets and the seals made through the link since the Inspector's last pass, in order: a
    /// set as its entity's ID, component, path and value, and a seal as `None`. For tests.
    #[doc(hidden)]
    pub fn pending(&self) -> Vec<Option<(EntityId, String, String, Value)>> {
        let linked = self.0.borrow();
        let pending = linked.pending.iter().map(|linking| match linking {
            Linking::Set(id, edit) => Some((
                *id,
                edit.component.clone(),
                edit.path.clone(),
                edit.value.clone(),
            )),
            Linking::Seal => None,
        });
        pending.collect()
    }
}

/// Whether `path` is a field of a component's own, or a component of a vector that's one, as far
/// as its text tells: a name, and at most an axis after it.
fn own_field(path: &str) -> bool {
    let (field, axis) = path.split_once('.').unwrap_or((path, "x"));
    let named = !field.is_empty() && !field.contains('[');
    named && AXES.iter().any(|&(name, ..)| name == axis)
}

/// Whether `edit`, a link's, sets what a link may, as `world`'s types describe its component: a
/// field of the component's own that holds nothing the panels set at a path of its own, as a
/// struct, a list or an array does; or a component of a vector that's one.
pub(super) fn links_field(world: &World, edit: &Edit) -> bool {
    let types = world.types();
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
        (Type::Vec2 | Type::Vec3 | Type::Vec4, Some(axis)) => {
            AXES.iter().any(|&(named, ..)| named == axis)
        }
        (_, Some(_)) | (Type::List(_) | Type::Array(..), None) => false,
        (&Type::Named(index), None) => !matches!(types.info(index).shape, Shape::Struct { .. }),
        (_, None) => true,
    }
}
