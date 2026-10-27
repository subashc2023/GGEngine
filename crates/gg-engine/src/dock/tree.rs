//! Where the panels are: in the main window, a tree of splits and tab groups around the central
//! area, which stays the game's, or, in a dockspace, filling the main window, the game's view
//! among them in a panel of its own; elsewhere, groups that float over the main window or in
//! windows of their own. It's plain data, saved between runs.

use std::mem;

use egui::{Rect, Vec2};
use serde::{Deserialize, Serialize};

/// A node's identity while the app runs, which egui's ids and a group's window are made from.
/// Nodes get new ones each run.
pub(crate) type NodeId = u64;

/// The central area's id.
pub(crate) const CENTRAL: NodeId = 0;

/// The smallest a docked group gets across a split, in points, while there's room.
const MIN_SIZE: f32 = 48.0;

/// How thick a splitter's handle is, in points.
const HANDLE: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Axis {
    /// Side by side, the first on the left.
    Horizontal,
    /// One above the other, the first on top.
    Vertical,
}

impl Axis {
    /// The index of this axis's component in a point or vector.
    fn index(self) -> usize {
        match self {
            Self::Horizontal => 0,
            Self::Vertical => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

/// Where a dropped group goes, relative to a node: among its tabs, or beside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Zone {
    Center,
    Side(Side),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Node {
    /// Two nodes sharing a rect along `axis`, the first taking `ratio` of it.
    Split {
        #[serde(skip)]
        id: NodeId,
        axis: Axis,
        ratio: f32,
        children: Box<[Node; 2]>,
    },
    Tabs(Tabs),
    /// The game's area, which panels dock around. In a dockspace, the empty dock, which is the
    /// whole main tree when it's there at all.
    Central,
}

/// Panels sharing a rect, one showing at a time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Tabs {
    #[serde(skip)]
    pub(crate) id: NodeId,
    pub(crate) panels: Vec<String>,
    /// The panel that shows, as an index into `panels`.
    pub(crate) active: usize,
}

/// A group outside the main window's tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Floating {
    pub(crate) tabs: Tabs,
    pub(crate) place: Place,
    /// The place in a window of its own that the layout saves for a group that this run floats
    /// over the main window instead, for want of windows of their own, until the user moves or
    /// resizes it there.
    #[serde(skip)]
    pub(crate) kept: Option<Place>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Place {
    /// Over the main window: the group's top-left corner and size there, in points.
    Main { min: [f32; 2], size: [f32; 2] },
    /// In a window of its own: the window's outer top-left corner on the desktop, in physical
    /// pixels, and the size inside its frame, in points.
    Window { position: [i32; 2], size: [f32; 2] },
}

/// The whole dock.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Layout {
    pub(crate) main: Node,
    pub(crate) floating: Vec<Floating>,
    /// In a dockspace, the panel that shows the game's view: the dock fills the main window,
    /// without a central area for the game, as the layout's made or loaded for.
    #[serde(skip)]
    pub(crate) view: Option<&'static str>,
    #[serde(skip)]
    next_id: NodeId,
}

/// Where the main tree puts things this frame.
#[derive(Debug)]
pub(crate) struct Arrangement {
    /// The strips taken off the main window's edges, from the outside in: the side, and the
    /// size across it in points. Each is a split's side on the way to the central area.
    pub(crate) strips: Vec<(Side, f32)>,
    /// Each docked group with a panel to show, and its rect.
    pub(crate) groups: Vec<(NodeId, Rect)>,
    pub(crate) splitters: Vec<Splitter>,
    /// The central area, which is the game's. In a dockspace, the empty dock while the main tree
    /// holds no panels, which a dropped group fills, and otherwise nothing.
    pub(crate) central: Rect,
    /// The rect it's laid out in: the main window's, below its title bar. Groups floating over
    /// the main window stay in it.
    pub(crate) area: Rect,
}

/// The handle between a split's two sides.
#[derive(Debug)]
pub(crate) struct Splitter {
    pub(crate) split: NodeId,
    pub(crate) axis: Axis,
    /// The split's whole rect.
    pub(crate) rect: Rect,
    /// Where the split divides `rect` along its axis, as [`Splitter::ratio_at`] takes it.
    pub(crate) at: f32,
    /// Where the handle is grabbed: the gap between the sides. A strip's lies wholly inside the
    /// strip, so that the central area stays the game's.
    pub(crate) handle: Rect,
}

impl Splitter {
    /// The ratio that puts the line between the sides at `at`, along the split's axis, keeping
    /// both sides at least the minimum size where there's room; `None` for a split with no room
    /// to divide, which keeps its ratio.
    pub(crate) fn ratio_at(&self, at: f32) -> Option<f32> {
        let axis = self.axis.index();
        let extent = self.rect.size()[axis];
        (extent > 0.0).then(|| clamp_first(at - self.rect.min[axis], extent) / extent)
    }
}

impl Layout {
    /// A new dock: the central area alone, or, in a dockspace, the group of the panel `view` names,
    /// which shows the game's view, filling it.
    pub(crate) fn new(view: Option<&'static str>) -> Self {
        let mut layout = Self {
            main: Node::Central,
            floating: Vec::new(),
            view,
            next_id: CENTRAL + 1,
        };
        if let Some(view) = view {
            layout.main = Node::Tabs(Tabs {
                id: layout.new_id(),
                panels: vec![view.to_owned()],
                active: 0,
            });
        }
        layout
    }

    /// The layout as it's saved: with the places that floating groups keep in windows of their
    /// own.
    pub(crate) fn saved(&self) -> Self {
        let mut saved = self.clone();
        for floating in &mut saved.floating {
            if let Some(kept) = floating.kept.take() {
                floating.place = kept;
            }
        }
        saved
    }

    /// Gives every node a new id, as a layout read from a file needs.
    pub(crate) fn renumber(&mut self) {
        let mut next = CENTRAL + 1;
        let mut take = || {
            next += 1;
            next - 1
        };
        self.main.renumber(&mut take);
        for floating in &mut self.floating {
            floating.tabs.id = take();
        }
        self.next_id = take();
    }

    /// The group holding `panel`.
    pub(crate) fn find(&self, panel: &str) -> Option<NodeId> {
        self.main.find(panel).or_else(|| {
            let floating = self.floating.iter().find(|f| f.tabs.holds(panel))?;
            Some(floating.tabs.id)
        })
    }

    pub(crate) fn group(&self, id: NodeId) -> Option<&Tabs> {
        match self.main.get(id) {
            Some(Node::Tabs(tabs)) => Some(tabs),
            _ => Some(&self.floating(id)?.tabs),
        }
    }

    fn group_mut(&mut self, id: NodeId) -> Option<&mut Tabs> {
        if let Some(Node::Tabs(_)) = self.main.get(id) {
            let Some(Node::Tabs(tabs)) = self.main.get_mut(id) else {
                unreachable!("the node was just found")
            };
            return Some(tabs);
        }
        Some(&mut self.floating_mut(id)?.tabs)
    }

    pub(crate) fn floating(&self, id: NodeId) -> Option<&Floating> {
        self.floating.iter().find(|f| f.tabs.id == id)
    }

    pub(crate) fn floating_mut(&mut self, id: NodeId) -> Option<&mut Floating> {
        self.floating.iter_mut().find(|f| f.tabs.id == id)
    }

    /// Adds `panel` in a group of its own, floating at `place`, and returns the group.
    pub(crate) fn float(&mut self, panel: String, place: Place) -> NodeId {
        let id = self.new_id();
        self.floating.push(Floating {
            tabs: Tabs {
                id,
                panels: vec![panel],
                active: 0,
            },
            place,
            kept: None,
        });
        id
    }

    /// Takes `panel` out of its group and floats it at `place`, in a new group, which it
    /// returns. A group left empty stays until [`Layout::prune`].
    pub(crate) fn detach(&mut self, panel: &str, place: Place) -> NodeId {
        if let Some(tabs) = self.find(panel).and_then(|id| self.group_mut(id)) {
            tabs.remove(panel);
        }
        self.float(panel.to_owned(), place)
    }

    /// Makes `panel` the one its group shows.
    pub(crate) fn activate(&mut self, panel: &str) {
        if let Some(tabs) = self.find(panel).and_then(|id| self.group_mut(id))
            && let Some(i) = tabs.panels.iter().position(|p| p == panel)
        {
            tabs.active = i;
        }
    }

    /// Moves the panels of floating group `group` to `target`: among the tabs of a group, or
    /// beside a node of the main tree, splitting it. Returns `false`, moving nothing, if `zone`
    /// isn't one of those for `target`: the central area takes panels only beside it, and a
    /// floating group only among its tabs. In a dockspace, the central area is the empty dock,
    /// which takes the group in its place while the main tree holds no panels, and nothing beside
    /// it.
    pub(crate) fn dock(&mut self, group: NodeId, target: NodeId, zone: Zone) -> bool {
        let dockspace = self.view.is_some();
        let fits = match zone {
            Zone::Center if target == CENTRAL => dockspace && !self.main.has_panels(),
            Zone::Center => self.group(target).is_some(),
            Zone::Side(_) => !(dockspace && target == CENTRAL) && self.main.get(target).is_some(),
        };
        let Some(from) = self.floating.iter().position(|f| f.tabs.id == group) else {
            return false;
        };
        if !fits || group == target {
            return false;
        }
        let moved = self.floating.remove(from).tabs;
        match zone {
            // Groups the drag emptied, which it hasn't pruned yet, go with it.
            Zone::Center if target == CENTRAL => self.main = Node::Tabs(moved),
            Zone::Center => {
                let tabs = self.group_mut(target).expect("checked above");
                tabs.active = tabs.panels.len() + moved.active;
                tabs.panels.extend(moved.panels);
            }
            Zone::Side(side) => {
                let id = self.new_id();
                let share = self.share(target);
                let node = self.main.get_mut(target).expect("checked above");
                let old = mem::replace(node, Node::Central);
                let new = Node::Tabs(moved);
                let (axis, ratio, children) = match side {
                    Side::Left => (Axis::Horizontal, share, [new, old]),
                    Side::Right => (Axis::Horizontal, 1.0 - share, [old, new]),
                    Side::Top => (Axis::Vertical, share, [new, old]),
                    Side::Bottom => (Axis::Vertical, 1.0 - share, [old, new]),
                };
                *node = Node::Split {
                    id,
                    axis,
                    ratio,
                    children: Box::new(children),
                };
            }
        }
        true
    }

    /// Drops the groups left without panels. A docked one's split goes too, its other side
    /// taking its place, and a main tree left without any, as only a dockspace's can be, is the
    /// empty dock again.
    pub(crate) fn prune(&mut self) {
        self.floating.retain(|f| !f.tabs.panels.is_empty());
        if self.main.prune() {
            self.main = Node::Central;
        }
    }

    pub(crate) fn set_ratio(&mut self, split: NodeId, to: f32) {
        if let Some(Node::Split { ratio, .. }) = self.main.get_mut(split) {
            *ratio = to;
        }
    }

    /// Lays out the main tree in `rect`, the main window's below its title bar, where `shown`
    /// says which panels show this frame. A group with none takes no room.
    pub(crate) fn arrange(&self, rect: Rect, shown: &impl Fn(&str) -> bool) -> Arrangement {
        // A dockspace's main tree holds no panels when it's the empty dock, or mid-drag, when the
        // drag has emptied the groups it holds and not yet pruned them.
        let central = if self.main.has_panels() {
            Rect::NOTHING
        } else {
            rect
        };
        let mut arrangement = Arrangement {
            strips: Vec::new(),
            groups: Vec::new(),
            splitters: Vec::new(),
            central,
            area: rect,
        };
        // Strips come off the edges on the way to the central area, which a dockspace hasn't.
        let peel = self.main.has_central();
        self.main.arrange(rect, shown, peel, &mut arrangement);
        arrangement
    }

    /// What's wrong with a layout read from a file, if anything.
    pub(crate) fn check(&self) -> Result<(), String> {
        let mut centrals = 0;
        let mut panels = Vec::new();
        let mut problem = None;
        self.main.walk(&mut |node| match node {
            Node::Central => centrals += 1,
            Node::Tabs(tabs) => panels.push(tabs),
            Node::Split { ratio, .. } => {
                // A handle dragged to the edge of a split too small for both sides' minimum
                // puts its ratio at 0 or 1.
                if !(0.0..=1.0).contains(ratio) {
                    problem
                        .get_or_insert(format!("a split's ratio is {ratio}, not between 0 and 1"));
                }
            }
        });
        if let Some(problem) = problem {
            return Err(problem);
        }
        match self.view {
            // The empty dock, which is the whole tree when it's there at all.
            Some(_) if centrals > 0 && self.main != Node::Central => {
                return Err(
                    "a dockspace's main tree has a central area among its groups".to_owned(),
                );
            }
            None if centrals != 1 => {
                return Err(format!("the main tree has {centrals} central areas, not 1"));
            }
            _ => {}
        }
        panels.extend(self.floating.iter().map(|f| &f.tabs));
        let mut names: Vec<&str> = panels
            .iter()
            .flat_map(|tabs| tabs.panels.iter().map(String::as_str))
            .collect();
        names.sort_unstable();
        if let Some(pair) = names.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(format!("panel {:?} is in two places", pair[0]));
        }
        if let Some(tabs) = panels
            .iter()
            .find(|tabs| tabs.active >= tabs.panels.len().max(1))
        {
            return Err(format!(
                "a group of {} panels shows panel {}",
                tabs.panels.len(),
                tabs.active
            ));
        }
        for floating in &self.floating {
            let size = match floating.place {
                Place::Main { min, size } if min.iter().all(|c| c.is_finite()) => size,
                Place::Window { size, .. } => size,
                Place::Main { .. } => return Err("a floating group is at no position".to_owned()),
            };
            if !size.iter().all(|c| c.is_finite() && *c > 0.0) {
                return Err(format!("a floating group is {size:?} points"));
            }
        }
        Ok(())
    }

    /// The share of `target`'s rect that a group docked beside it takes: a quarter of the central
    /// area, or of the group of a dockspace's view, so that the game stays in view, and half of
    /// anything else.
    pub(crate) fn share(&self, target: NodeId) -> f32 {
        if target == CENTRAL || self.holds_view(target) {
            0.25
        } else {
            0.5
        }
    }

    /// Whether group `id` holds the panel that shows a dockspace's view.
    pub(crate) fn holds_view(&self, id: NodeId) -> bool {
        (self.view).is_some_and(|view| self.group(id).is_some_and(|tabs| tabs.holds(view)))
    }

    fn new_id(&mut self) -> NodeId {
        self.next_id += 1;
        self.next_id - 1
    }
}

impl Tabs {
    pub(crate) fn holds(&self, panel: &str) -> bool {
        self.panels.iter().any(|p| p == panel)
    }

    /// The panel the group shows, of those `shown`: its active one, or else its first.
    pub(crate) fn showing(&self, shown: &impl Fn(&str) -> bool) -> Option<&str> {
        let active = self.panels.get(self.active).filter(|p| shown(p));
        active
            .or_else(|| self.panels.iter().find(|p| shown(p)))
            .map(String::as_str)
    }

    /// Takes `panel` out; the panel after it shows if it was showing.
    fn remove(&mut self, panel: &str) {
        if let Some(i) = self.panels.iter().position(|p| p == panel) {
            self.panels.remove(i);
            if self.active > i || self.active == self.panels.len() {
                self.active = self.active.saturating_sub(1);
            }
        }
    }
}

impl Node {
    fn walk<'a>(&'a self, visit: &mut impl FnMut(&'a Node)) {
        visit(self);
        if let Node::Split { children, .. } = self {
            children[0].walk(visit);
            children[1].walk(visit);
        }
    }

    fn renumber(&mut self, take: &mut impl FnMut() -> NodeId) {
        match self {
            Node::Split { id, children, .. } => {
                *id = take();
                children[0].renumber(take);
                children[1].renumber(take);
            }
            Node::Tabs(tabs) => tabs.id = take(),
            Node::Central => {}
        }
    }

    fn id(&self) -> NodeId {
        match self {
            Node::Split { id, .. } => *id,
            Node::Tabs(tabs) => tabs.id,
            Node::Central => CENTRAL,
        }
    }

    fn get(&self, id: NodeId) -> Option<&Node> {
        if self.id() == id {
            return Some(self);
        }
        let Node::Split { children, .. } = self else {
            return None;
        };
        children[0].get(id).or_else(|| children[1].get(id))
    }

    fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        if self.id() == id {
            return Some(self);
        }
        let Node::Split { children, .. } = self else {
            return None;
        };
        let [first, second] = &mut **children;
        first.get_mut(id).or_else(|| second.get_mut(id))
    }

    fn find(&self, panel: &str) -> Option<NodeId> {
        match self {
            Node::Split { children, .. } => {
                children[0].find(panel).or_else(|| children[1].find(panel))
            }
            Node::Tabs(tabs) => tabs.holds(panel).then_some(tabs.id),
            Node::Central => None,
        }
    }

    fn has_central(&self) -> bool {
        match self {
            Node::Split { children, .. } => children.iter().any(Node::has_central),
            Node::Tabs(_) => false,
            Node::Central => true,
        }
    }

    /// Whether the node holds any panel, shown or not.
    fn has_panels(&self) -> bool {
        match self {
            Node::Split { children, .. } => children.iter().any(Node::has_panels),
            Node::Tabs(tabs) => !tabs.panels.is_empty(),
            Node::Central => false,
        }
    }

    /// Whether the node has nothing to show, so that it takes no room.
    fn is_empty(&self, shown: &impl Fn(&str) -> bool) -> bool {
        match self {
            Node::Split { children, .. } => children.iter().all(|child| child.is_empty(shown)),
            Node::Tabs(tabs) => !tabs.panels.iter().any(|p| shown(p)),
            Node::Central => false,
        }
    }

    /// Returns whether this node is a group without panels, for its parent to drop.
    fn prune(&mut self) -> bool {
        let empty = match self {
            Node::Split { children, .. } => [children[0].prune(), children[1].prune()],
            Node::Tabs(tabs) => return tabs.panels.is_empty(),
            Node::Central => return false,
        };
        match empty {
            [false, false] => false,
            [true, true] => true,
            [first_empty, _] => {
                let Node::Split { children, .. } = mem::replace(self, Node::Central) else {
                    unreachable!("the node is a split")
                };
                let [first, second] = *children;
                *self = if first_empty { second } else { first };
                false
            }
        }
    }

    /// `peel` is set on the way from the root to the central area, where each split's other
    /// side is a strip along an edge.
    fn arrange(
        &self,
        rect: Rect,
        shown: &impl Fn(&str) -> bool,
        peel: bool,
        out: &mut Arrangement,
    ) {
        match self {
            Node::Central => out.central = rect,
            Node::Tabs(tabs) => {
                if tabs.panels.iter().any(|p| shown(p)) {
                    out.groups.push((tabs.id, rect));
                }
            }
            Node::Split {
                id,
                axis,
                ratio,
                children,
            } => {
                let [first, second] = &**children;
                match (first.is_empty(shown), second.is_empty(shown)) {
                    (true, true) => {}
                    (false, true) => first.arrange(rect, shown, peel, out),
                    (true, false) => second.arrange(rect, shown, peel, out),
                    (false, false) => {
                        let i = axis.index();
                        let at = rect.min[i] + clamp_first(rect.size()[i] * ratio, rect.size()[i]);
                        // The sides leave a gap between them for the handle: for a strip, inside
                        // the strip, so that the central area stays the game's.
                        let strip_first = peel && second.has_central();
                        let strip_second = peel && first.has_central();
                        let (from, to) = match (strip_first, strip_second) {
                            (true, _) => (at - HANDLE, at),
                            (_, true) => (at, at + HANDLE),
                            _ => (at - HANDLE / 2.0, at + HANDLE / 2.0),
                        };
                        // A side thinner than its part of the gap ends at the split's edge.
                        let (mut first_rect, mut handle, mut second_rect) = (rect, rect, rect);
                        first_rect.max[i] = from.max(rect.min[i]);
                        (handle.min[i], handle.max[i]) = (from, to);
                        second_rect.min[i] = to.min(rect.max[i]);
                        if peel {
                            let strip = match (axis, strip_first) {
                                (Axis::Horizontal, true) => (Side::Left, at - rect.min.x),
                                (Axis::Horizontal, false) => (Side::Right, rect.max.x - at),
                                (Axis::Vertical, true) => (Side::Top, at - rect.min.y),
                                (Axis::Vertical, false) => (Side::Bottom, rect.max.y - at),
                            };
                            out.strips.push(strip);
                        }
                        out.splitters.push(Splitter {
                            split: *id,
                            axis: *axis,
                            rect,
                            at,
                            handle,
                        });
                        first.arrange(first_rect, shown, strip_second, out);
                        second.arrange(second_rect, shown, strip_first, out);
                    }
                }
            }
        }
    }
}

/// Where a split's line goes, `first` along its `extent`, keeping both sides the minimum size
/// where there's room.
fn clamp_first(first: f32, extent: f32) -> f32 {
    if extent >= 2.0 * MIN_SIZE {
        first.clamp(MIN_SIZE, extent - MIN_SIZE)
    } else {
        // A window smaller than its title bar leaves the dock a rect without an area.
        first.clamp(0.0, extent.max(0.0))
    }
}

impl Place {
    /// The size of what's inside the group, in points.
    pub(crate) fn size(&self) -> Vec2 {
        let (Place::Main { size, .. } | Place::Window { size, .. }) = *self;
        size.into()
    }
}

#[cfg(test)]
mod tests {
    use egui::{pos2, vec2};

    use super::*;

    const AT: Place = Place::Main {
        min: [10.0, 10.0],
        size: [200.0, 100.0],
    };

    /// Docks `panel` as a strip on the central area's `side`, and returns its group.
    fn strip(layout: &mut Layout, panel: &str, side: Side) -> NodeId {
        let group = layout.float(panel.to_owned(), AT);
        assert!(layout.dock(group, CENTRAL, Zone::Side(side)));
        group
    }

    fn all(_: &str) -> bool {
        true
    }

    #[test]
    fn docking_among_tabs_adds_a_tab_and_beside_a_node_splits_it() {
        let mut layout = Layout::new(None);
        let a = strip(&mut layout, "A", Side::Left);
        let b = layout.float("B".to_owned(), AT);
        assert!(layout.dock(b, a, Zone::Center));
        let c = layout.float("C".to_owned(), AT);
        assert!(layout.dock(c, a, Zone::Side(Side::Bottom)));
        let tabs = |panels: &[&str], active| {
            Node::Tabs(Tabs {
                id: 0,
                panels: panels.iter().map(|p| (*p).to_owned()).collect(),
                active,
            })
        };
        let mut expected = Layout {
            main: Node::Split {
                id: 0,
                axis: Axis::Horizontal,
                ratio: 0.25,
                children: Box::new([
                    Node::Split {
                        id: 0,
                        axis: Axis::Vertical,
                        ratio: 0.5,
                        children: Box::new([tabs(&["A", "B"], 1), tabs(&["C"], 0)]),
                    },
                    Node::Central,
                ]),
            },
            floating: Vec::new(),
            view: None,
            next_id: 0,
        };
        expected.renumber();
        layout.renumber();
        assert_eq!(layout, expected);
        assert_eq!(layout.find("B"), layout.find("A"));
    }

    #[test]
    fn the_central_area_takes_panels_only_beside_it_and_floating_groups_only_as_tabs() {
        let mut layout = Layout::new(None);
        let a = layout.float("A".to_owned(), AT);
        let b = layout.float("B".to_owned(), AT);
        assert!(!layout.dock(a, CENTRAL, Zone::Center));
        assert!(!layout.dock(a, b, Zone::Side(Side::Left)));
        assert!(!layout.dock(a, a, Zone::Center));
        assert_eq!(layout.main, Node::Central);
        assert_eq!(layout.floating.len(), 2);
        assert!(layout.dock(a, b, Zone::Center));
        assert_eq!(layout.floating.len(), 1);
        assert_eq!(layout.floating[0].tabs.panels, ["B", "A"]);
    }

    #[test]
    fn a_group_left_empty_goes_with_its_split() {
        let mut layout = Layout::new(None);
        let a = strip(&mut layout, "A", Side::Left);
        strip(&mut layout, "B", Side::Bottom);
        let moved = layout.detach("A", AT);
        // The emptied group keeps its place until pruned.
        assert_eq!(layout.group(a).map(|tabs| tabs.panels.len()), Some(0));
        layout.prune();
        assert_eq!(layout.group(a), None);
        let Node::Split { children, .. } = &layout.main else {
            panic!("B's split stays: {:?}", layout.main);
        };
        assert_eq!(
            children[1],
            Node::Tabs(Tabs {
                id: layout.find("B").unwrap(),
                panels: vec!["B".to_owned()],
                active: 0
            })
        );
        assert_eq!(layout.find("A"), Some(moved));
        // Docking it again and emptying the other strip leaves A's alone.
        assert!(layout.dock(moved, CENTRAL, Zone::Side(Side::Right)));
        layout.detach("B", AT);
        layout.prune();
        assert!(
            matches!(&layout.main, Node::Split { children, .. } if children[0] == Node::Central)
        );
    }

    #[test]
    fn a_group_whose_panels_are_hidden_takes_no_room() {
        let mut layout = Layout::new(None);
        strip(&mut layout, "A", Side::Left);
        let b = strip(&mut layout, "B", Side::Top);
        let window = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let hidden = layout.arrange(window, &|panel| panel != "A");
        assert_eq!(
            hidden.groups,
            [(b, Rect::from_min_max(pos2(0.0, 0.0), pos2(800.0, 142.0)))]
        );
        assert_eq!(
            hidden.central,
            Rect::from_min_max(pos2(0.0, 150.0), pos2(800.0, 600.0))
        );
    }

    #[test]
    fn strips_come_off_the_edges_from_the_outside_in_at_no_less_than_the_minimum() {
        let mut layout = Layout::new(None);
        let a = strip(&mut layout, "A", Side::Left);
        let b = strip(&mut layout, "B", Side::Bottom);
        let window = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 600.0));
        let arranged = layout.arrange(window, &all);
        assert_eq!(
            arranged.strips,
            [(Side::Left, 250.0), (Side::Bottom, 150.0)]
        );
        assert_eq!(
            arranged.central,
            Rect::from_min_max(pos2(250.0, 0.0), pos2(1000.0, 450.0))
        );
        assert_eq!(
            arranged.groups,
            [
                (a, Rect::from_min_max(pos2(0.0, 0.0), pos2(242.0, 600.0))),
                (
                    b,
                    Rect::from_min_max(pos2(250.0, 458.0), pos2(1000.0, 600.0))
                ),
            ]
        );
        // The handles stay inside the strips.
        let [left, bottom] = &arranged.splitters[..] else {
            panic!("two splitters: {:?}", arranged.splitters);
        };
        assert_eq!(
            left.handle,
            Rect::from_min_max(pos2(242.0, 0.0), pos2(250.0, 600.0))
        );
        assert_eq!(
            bottom.handle,
            Rect::from_min_max(pos2(250.0, 450.0), pos2(1000.0, 458.0))
        );

        // A handle dragged almost to the edge leaves the strip its minimum.
        let ratio = left.ratio_at(3.0).expect("the split has room");
        layout.set_ratio(left.split, ratio);
        assert_eq!(
            layout.arrange(window, &all).strips[0],
            (Side::Left, MIN_SIZE)
        );
    }

    #[test]
    fn a_handle_dragged_to_the_edge_of_a_narrow_split_saves_a_ratio_the_layout_takes() {
        let mut layout = Layout::new(None);
        strip(&mut layout, "A", Side::Left);
        let window = Rect::from_min_size(pos2(0.0, 0.0), vec2(80.0, 600.0));
        let arranged = layout.arrange(window, &all);
        let splitter = &arranged.splitters[0];
        for (at, edge) in [(-10.0, 0.0), (90.0, 1.0)] {
            assert_eq!(splitter.ratio_at(at), Some(edge));
            layout.set_ratio(splitter.split, edge);
            assert_eq!(layout.check(), Ok(()));
        }
        let squeezed = Rect::from_min_size(pos2(0.0, 0.0), vec2(0.0, 600.0));
        assert_eq!(
            layout.arrange(squeezed, &all).splitters[0].ratio_at(0.0),
            None
        );
    }

    #[test]
    fn splits_squeezed_past_their_handles_lay_out_inside_the_window() {
        // Splits along one axis inside one another, with ratios a saved layout may have: the
        // middle one's first side is thinner than its part of the handle's gap.
        let tabs = |panel: &str| {
            Node::Tabs(Tabs {
                id: 0,
                panels: vec![panel.to_owned()],
                active: 0,
            })
        };
        let split = |ratio, children| Node::Split {
            id: 0,
            axis: Axis::Horizontal,
            ratio,
            children: Box::new(children),
        };
        let mut layout = Layout {
            main: split(
                0.25,
                [
                    split(0.001, [split(0.5, [tabs("A"), tabs("B")]), tabs("C")]),
                    Node::Central,
                ],
            ),
            floating: Vec::new(),
            view: None,
            next_id: 0,
        };
        layout.renumber();
        assert_eq!(layout.check(), Ok(()));
        let window = Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 100.0));
        let arranged = layout.arrange(window, &all);
        assert_eq!(arranged.groups.len(), 3);
        for (_, rect) in arranged.groups {
            assert!(
                window.contains_rect(rect) && !rect.is_negative(),
                "{rect:?}"
            );
        }
    }

    /// The panel that shows the view of the tests' dockspaces.
    const VIEW: &str = "View";

    #[test]
    fn a_dockspace_fills_with_groups_and_peels_no_strips() {
        let mut layout = Layout::new(Some(VIEW));
        assert_eq!(layout.check(), Ok(()));
        let view = layout.find(VIEW).expect("the view's group fills the dock");
        // Beside the view's group, a group takes a quarter of it, as beside the central area.
        let a = layout.float("A".to_owned(), AT);
        assert!(layout.dock(a, view, Zone::Side(Side::Right)));
        let b = layout.float("B".to_owned(), AT);
        assert!(layout.dock(b, a, Zone::Side(Side::Bottom)));
        assert_eq!(layout.check(), Ok(()));
        let window = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 600.0));
        let arranged = layout.arrange(window, &all);
        // No edge is a strip, and each gap is in the middle of its split.
        assert!(arranged.strips.is_empty(), "{:?}", arranged.strips);
        assert_eq!(arranged.central, Rect::NOTHING);
        assert_eq!(
            arranged.groups,
            [
                (view, Rect::from_min_max(pos2(0.0, 0.0), pos2(746.0, 600.0))),
                (a, Rect::from_min_max(pos2(754.0, 0.0), pos2(1000.0, 296.0))),
                (
                    b,
                    Rect::from_min_max(pos2(754.0, 304.0), pos2(1000.0, 600.0))
                ),
            ]
        );
    }

    #[test]
    fn a_dockspace_emptied_is_the_central_area_which_takes_a_group_in_its_place() {
        let mut layout = Layout::new(Some(VIEW));
        let window = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 600.0));
        // While the dock holds panels, its central area takes none.
        let a = layout.float("A".to_owned(), AT);
        assert!(!layout.dock(a, CENTRAL, Zone::Center));
        assert!(!layout.dock(a, CENTRAL, Zone::Side(Side::Left)));
        // Mid-drag, before the drag's end prunes it, the view's group is empty, and so is the dock.
        let view = layout.detach(VIEW, AT);
        assert_eq!(layout.arrange(window, &all).central, window);
        assert!(!layout.dock(view, CENTRAL, Zone::Side(Side::Left)));
        assert!(layout.dock(view, CENTRAL, Zone::Center));
        layout.prune();
        assert!(matches!(&layout.main, Node::Tabs(tabs) if tabs.id == view));
        // Emptied and pruned, the dock is the central area alone, which takes the group again.
        let view = layout.detach(VIEW, AT);
        layout.prune();
        assert_eq!(layout.main, Node::Central);
        assert_eq!(layout.check(), Ok(()));
        assert_eq!(layout.arrange(window, &all).central, window);
        assert!(layout.dock(view, CENTRAL, Zone::Center));
        assert_eq!(layout.find(VIEW), Some(view));
        assert_eq!(layout.arrange(window, &all).groups, [(view, window)]);
    }

    #[test]
    fn a_dockspace_has_the_central_area_only_as_its_whole_tree() {
        let tabs = Node::Tabs(Tabs {
            id: 0,
            panels: vec![VIEW.to_owned()],
            active: 0,
        });
        let mut layout = Layout {
            main: Node::Split {
                id: 0,
                axis: Axis::Horizontal,
                ratio: 0.5,
                children: Box::new([tabs.clone(), Node::Central]),
            },
            floating: Vec::new(),
            view: Some(VIEW),
            next_id: 0,
        };
        layout.renumber();
        assert!(layout.check().is_err());
        for main in [tabs.clone(), Node::Central] {
            layout.main = main;
            layout.renumber();
            assert_eq!(layout.check(), Ok(()));
        }
        // A game's has one, always.
        layout.main = tabs;
        layout.view = None;
        assert!(layout.check().is_err());
    }
}
