//! Where the panels are. In the main window: a tree of splits and tab groups around the central
//! area, which stays the game's. In a dockspace the tree fills the window, and the game's view is
//! a panel in it. Elsewhere: groups floating over the main window or in windows of their own.
//! Also the panels the user closed, which keep their places. Plain data, saved between runs; it
//! places the panels an app docks as they first show.

use std::mem;

use egui::{Rect, Vec2};
use serde::{Deserialize, Serialize};

use super::Docked;

/// A node's identity, new each run; egui's ids and a group's window derive from it.
pub(crate) type NodeId = u64;

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
    fn index(self) -> usize {
        match self {
            Self::Horizontal => 0,
            Self::Vertical => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

/// Where a dropped group goes: among a node's tabs, or beside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Zone {
    Center,
    Side(Side),
}

impl Docked {
    fn target(self) -> &'static str {
        let (Self::LeftOf(target, _)
        | Self::RightOf(target, _)
        | Self::Above(target, _)
        | Self::Below(target, _)
        | Self::With(target)) = self;
        target
    }

    fn zone(self) -> Zone {
        match self {
            Self::LeftOf(..) => Zone::Side(Side::Left),
            Self::RightOf(..) => Zone::Side(Side::Right),
            Self::Above(..) => Zone::Side(Side::Top),
            Self::Below(..) => Zone::Side(Side::Bottom),
            Self::With(_) => Zone::Center,
        }
    }

    /// The share of its target's place it takes beside it, clamped to 0..=1; none among tabs.
    fn share(self) -> Option<f32> {
        match self {
            Self::LeftOf(_, share)
            | Self::RightOf(_, share)
            | Self::Above(_, share)
            | Self::Below(_, share) => Some(share.clamp(0.0, 1.0)),
            Self::With(_) => None,
        }
    }
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
    /// The game's area, which panels dock around. In a dockspace, the empty dock: the whole main
    /// tree.
    Central,
}

/// Panels sharing a rect, one showing at a time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Tabs {
    #[serde(skip)]
    pub(crate) id: NodeId,
    pub(crate) panels: Vec<String>,
    /// The showing panel's index in `panels`.
    pub(crate) active: usize,
}

/// A group outside the main window's tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Floating {
    pub(crate) tabs: Tabs,
    pub(crate) place: Place,
    /// The window place kept for a group floated over the main window for want of windows, until
    /// the user moves or resizes it there.
    #[serde(skip)]
    pub(crate) kept: Option<Place>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Place {
    /// Over the main window: the group's top-left corner and size, in points.
    Main { min: [f32; 2], size: [f32; 2] },
    /// In its own window: the outer top-left corner on the desktop in physical pixels, and the
    /// size inside the frame in points.
    Window { position: [i32; 2], size: [f32; 2] },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Layout {
    pub(crate) main: Node,
    pub(crate) floating: Vec<Floating>,
    /// Panels the user closed: shown nowhere, keeping their places, until opened. Older layouts
    /// have none.
    #[serde(default)]
    pub(crate) closed: Vec<String>,
    /// Sides of the central area or a dockspace's view whose panels the user hid, as
    /// [`Layout::side`] finds them: shown nowhere, keeping their places, until shown again. Older
    /// layouts have none.
    #[serde(default)]
    pub(crate) hidden: Vec<Side>,
    /// In a dockspace, the panel showing the game's view; the dock fills the main window, with no
    /// central area. Set on making or loading.
    #[serde(skip)]
    pub(crate) view: Option<&'static str>,
    #[serde(skip)]
    next_id: NodeId,
}

/// Where the main tree puts things this frame.
#[derive(Debug)]
pub(crate) struct Arrangement {
    /// Strips off the main window's edges, outside in: the side and its size across, in points.
    /// Each is a split's side on the way to the central area.
    pub(crate) strips: Vec<(Side, f32)>,
    /// Each docked group with a panel to show, and its rect.
    pub(crate) groups: Vec<(NodeId, Rect)>,
    pub(crate) splitters: Vec<Splitter>,
    /// The game's central area. In a dockspace, the empty dock while the main tree shows no panel,
    /// for a dropped group to fill; otherwise nothing.
    pub(crate) central: Rect,
    /// The rect laid out in: the main window's below its title bar. Floating groups stay in it.
    pub(crate) area: Rect,
}

/// The handle between a split's two sides.
#[derive(Debug)]
pub(crate) struct Splitter {
    pub(crate) split: NodeId,
    pub(crate) axis: Axis,
    pub(crate) rect: Rect,
    /// Where the split divides `rect` along its axis, as [`Splitter::ratio_at`] takes it.
    pub(crate) at: f32,
    /// Where the handle is grabbed: the gap between the sides. A strip's lies wholly inside it,
    /// so the central area stays the game's.
    pub(crate) handle: Rect,
}

impl Splitter {
    /// The ratio that puts the line between the sides at `at` along the axis, keeping both sides
    /// at least the minimum size where there's room. `None` for a split with no room: it keeps
    /// its ratio.
    pub(crate) fn ratio_at(&self, at: f32) -> Option<f32> {
        let axis = self.axis.index();
        let extent = self.rect.size()[axis];
        (extent > 0.0).then(|| clamp_first(at - self.rect.min[axis], extent) / extent)
    }
}

impl Layout {
    /// A new dock: the central area alone, or in a dockspace, the `view` panel's group filling it.
    pub(crate) fn new(view: Option<&'static str>) -> Self {
        let mut layout = Self {
            main: Node::Central,
            floating: Vec::new(),
            closed: Vec::new(),
            hidden: Vec::new(),
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

    /// The layout as saved: floating groups at their kept window places.
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
        self.renumber_from(CENTRAL + 1);
    }

    /// Takes `other`'s place, numbering its nodes after this one's so no id returns: a window is
    /// named by its group's id, so a new group would get an old one's window.
    pub(crate) fn replace(&mut self, mut other: Layout) {
        other.renumber_from(self.next_id);
        *self = other;
    }

    fn renumber_from(&mut self, mut next: NodeId) {
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

    /// Adds `panel` in a new group floating at `place`, returned.
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

    /// Floats `panel` out of its group at `place`, in a new group, returned. An emptied group
    /// stays until [`Layout::prune`].
    pub(crate) fn detach(&mut self, panel: &str, place: Place) -> NodeId {
        if let Some(tabs) = self.find(panel).and_then(|id| self.group_mut(id)) {
            tabs.remove(panel);
        }
        self.float(panel.to_owned(), place)
    }

    /// Closes `panel`: shown nowhere, keeping its place, until opened. Returns whether it closed.
    /// A dockspace's view never does: nothing else shows the game.
    pub(crate) fn close(&mut self, panel: &str) -> bool {
        if self.view == Some(panel) {
            return false;
        }
        if !self.is_closed(panel) {
            self.closed.push(panel.to_owned());
        }
        true
    }

    pub(crate) fn open(&mut self, panel: &str) {
        self.closed.retain(|closed| closed != panel);
    }

    pub(crate) fn is_closed(&self, panel: &str) -> bool {
        self.closed.iter().any(|closed| closed == panel)
    }

    /// The side of the central area, or a dockspace's view, `panel` is docked on, across a split
    /// on the way to it from the root. `None` for a panel among its tabs, floating or absent, and
    /// in a dockspace whose view isn't docked.
    pub(crate) fn side(&self, panel: &str) -> Option<Side> {
        let leads = |node: &Node| match self.view {
            Some(view) => node.find(view).is_some(),
            None => node.has_central(),
        };
        let mut node = &self.main;
        while let Node::Split { axis, children, .. } = node {
            let [first, second] = &**children;
            // `after`: the panels away from it are right of it or below it.
            let (toward, away, after) = if leads(first) {
                (first, second, true)
            } else if leads(second) {
                (second, first, false)
            } else {
                return None;
            };
            if away.find(panel).is_some() {
                return Some(match (axis, after) {
                    (Axis::Horizontal, false) => Side::Left,
                    (Axis::Horizontal, true) => Side::Right,
                    (Axis::Vertical, false) => Side::Top,
                    (Axis::Vertical, true) => Side::Bottom,
                });
            }
            node = toward;
        }
        None
    }

    pub(crate) fn is_hidden(&self, panel: &str) -> bool {
        (self.side(panel)).is_some_and(|side| self.hidden.contains(&side))
    }

    pub(crate) fn set_hidden(&mut self, side: Side, hidden: bool) {
        self.hidden.retain(|&other| other != side);
        if hidden {
            self.hidden.push(side);
        }
    }

    /// Adds `panel`, not yet held, where the app's `docked` says: beside the named panel's
    /// main-tree group, taking the app's share, or among its tabs, which keep their showing tab.
    /// A target the layout lacks stands for its own target, and so on; a panel beside that takes
    /// the [default share](Layout::share), the app's being of the panel it names. A panel the app
    /// doesn't place, or whose target floats or comes to none, floats at `place`.
    pub(crate) fn add(&mut self, panel: String, place: Place, docked: &[(&'static str, Docked)]) {
        let to = self.docking(&panel, docked);
        let group = self.float(panel, place);
        let Some((target, zone, share)) = to else {
            return;
        };
        let active = self.group(target).map(|tabs| tabs.active);
        let share = share.unwrap_or_else(|| self.share(target));
        let moved = self.dock_taking(group, target, zone, share);
        debug_assert!(
            moved,
            "a group of the main tree takes panels beside it and among its tabs"
        );
        if zone == Zone::Center
            && let Some(active) = active
            && let Some(tabs) = self.group_mut(target)
        {
            tabs.active = active;
        }
    }

    /// Where `docked` docks `panel`: the main-tree group of the panel it names or its stand-in,
    /// the zone, and the app's share if the named panel is the target.
    fn docking(
        &self,
        panel: &str,
        docked: &[(&'static str, Docked)],
    ) -> Option<(NodeId, Zone, Option<f32>)> {
        let entry = |panel: &str| {
            docked
                .iter()
                .find_map(|&(p, how)| (p == panel).then_some(how))
        };
        let how = entry(panel)?;
        let mut target = how.target();
        let mut share = how.share();
        // At most one step per app panel, so targets standing for each other come to none.
        for _ in 0..=docked.len() {
            if let Some(group) = self.main.find(target) {
                return Some((group, how.zone(), share));
            }
            if self.find(target).is_some() {
                return None;
            }
            target = entry(target)?.target();
            share = None;
        }
        None
    }

    pub(crate) fn activate(&mut self, panel: &str) {
        if let Some(tabs) = self.find(panel).and_then(|id| self.group_mut(id))
            && let Some(i) = tabs.panels.iter().position(|p| p == panel)
        {
            tabs.active = i;
        }
    }

    /// Moves floating `group`'s panels to `target`: among a group's tabs, or beside a main tree
    /// node, splitting it. Returns `false`, moving nothing, if `zone` doesn't fit `target`: the
    /// central area takes panels only beside it, a floating group only among its tabs. In a
    /// dockspace the central area is the empty dock, offered while the main tree shows no panel.
    /// It takes the group among its tabs alone, filling the dock: in its place, or beside the
    /// main tree's panels, which take no room until one shows again, then the usual
    /// [share](Layout::share).
    pub(crate) fn dock(&mut self, group: NodeId, target: NodeId, zone: Zone) -> bool {
        let share = self.share(target);
        self.dock_taking(group, target, zone, share)
    }

    /// Like [`dock`](Self::dock), but beside `target` taking `share` of its place.
    fn dock_taking(&mut self, group: NodeId, target: NodeId, zone: Zone, share: f32) -> bool {
        let dockspace = self.view.is_some();
        let fits = match zone {
            Zone::Center if target == CENTRAL => dockspace,
            Zone::Center => self.group(target).is_some(),
            Zone::Side(_) => !(dockspace && target == CENTRAL) && self.main.get(target).is_some(),
        };
        let Some(from) = self.floating.iter().position(|f| f.tabs.id == group) else {
            return false;
        };
        if !fits || group == target {
            return false;
        }
        let beside = self.share(group);
        let moved = self.floating.remove(from).tabs;
        match zone {
            // Groups the drag emptied and hasn't pruned yet go with it.
            Zone::Center if target == CENTRAL && !self.main.has_panels() => {
                self.main = Node::Tabs(moved);
            }
            Zone::Center if target == CENTRAL => {
                let id = self.new_id();
                let hidden = mem::replace(&mut self.main, Node::Central);
                self.main = Node::Split {
                    id,
                    axis: Axis::Horizontal,
                    ratio: beside,
                    children: Box::new([hidden, Node::Tabs(moved)]),
                };
            }
            Zone::Center => {
                let tabs = self.group_mut(target).expect("checked above");
                tabs.active = tabs.panels.len() + moved.active;
                tabs.panels.extend(moved.panels);
            }
            Zone::Side(side) => {
                let id = self.new_id();
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

    /// Drops groups without panels. A docked one's split goes too, its other side taking its
    /// place; a main tree left with none, only a dockspace's, is the empty dock again.
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

    /// Lays out the main tree in `rect`, the main window's below its title bar; `shown` says which
    /// panels show this frame. A group with none takes no room; a dockspace's main tree showing
    /// none is the empty dock, taking all of `rect`.
    pub(crate) fn arrange(&self, rect: Rect, shown: &impl Fn(&str) -> bool) -> Arrangement {
        // A dockspace's tree shows no panel as the empty dock: mid-drag, when the drag emptied its
        // groups before a prune, and while its panels are closed or unshown. A game's holds the
        // central area, never empty.
        let central = if self.main.is_empty(shown) {
            rect
        } else {
            Rect::NOTHING
        };
        let mut arrangement = Arrangement {
            strips: Vec::new(),
            groups: Vec::new(),
            splitters: Vec::new(),
            central,
            area: rect,
        };
        // Strips come off the edges on the way to the central area; a dockspace has none.
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
                // A handle dragged to the edge of a split too small for both minimums sets its
                // ratio to 0 or 1.
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
            // The empty dock is the whole tree.
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

    /// The share of `target`'s rect a group docked beside it takes: a quarter for the central area
    /// or a dockspace's view group, keeping the game in view, else half.
    pub(crate) fn share(&self, target: NodeId) -> f32 {
        if target == CENTRAL || self.holds_view(target) {
            0.25
        } else {
            0.5
        }
    }

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

    /// The panel shown: the active one if `shown`, else the first that is.
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

    fn has_panels(&self) -> bool {
        match self {
            Node::Split { children, .. } => children.iter().any(Node::has_panels),
            Node::Tabs(tabs) => !tabs.panels.is_empty(),
            Node::Central => false,
        }
    }

    /// Whether nothing shows, so it takes no room.
    fn is_empty(&self, shown: &impl Fn(&str) -> bool) -> bool {
        match self {
            Node::Split { children, .. } => children.iter().all(|child| child.is_empty(shown)),
            Node::Tabs(tabs) => !tabs.panels.iter().any(|p| shown(p)),
            Node::Central => false,
        }
    }

    /// Whether this is a group without panels, for its parent to drop.
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

    /// `peel` is set on the way from the root to the central area: each split's other side is a
    /// strip along an edge.
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
                        // The sides leave a gap for the handle; a strip's lies inside the strip, so
                        // the central area stays the game's.
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

/// Clamps a split's line `first` within `extent`, keeping both sides the minimum size where
/// there's room.
fn clamp_first(first: f32, extent: f32) -> f32 {
    if extent >= 2.0 * MIN_SIZE {
        first.clamp(MIN_SIZE, extent - MIN_SIZE)
    } else {
        // A window smaller than its title bar leaves the dock a rect without an area.
        first.clamp(0.0, extent.max(0.0))
    }
}

impl Place {
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
            closed: Vec::new(),
            hidden: Vec::new(),
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
        // Nested splits on one axis with ratios a saved layout may have: the middle one's first
        // side is thinner than its part of the gap.
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
            closed: Vec::new(),
            hidden: Vec::new(),
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

    const VIEW: &str = "View";

    #[test]
    fn a_dockspace_fills_with_groups_and_peels_no_strips() {
        let mut layout = Layout::new(Some(VIEW));
        assert_eq!(layout.check(), Ok(()));
        let view = layout.find(VIEW).expect("the view's group fills the dock");
        let a = layout.float("A".to_owned(), AT);
        assert!(layout.dock(a, view, Zone::Side(Side::Right)));
        let b = layout.float("B".to_owned(), AT);
        assert!(layout.dock(b, a, Zone::Side(Side::Bottom)));
        assert_eq!(layout.check(), Ok(()));
        let window = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 600.0));
        let arranged = layout.arrange(window, &all);
        // No edge is a strip; each gap is mid-split.
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
        // While the dock shows a panel it has no empty dock: the central area takes nothing beside
        // it.
        let a = layout.float("A".to_owned(), AT);
        assert_eq!(layout.arrange(window, &all).central, Rect::NOTHING);
        assert!(!layout.dock(a, CENTRAL, Zone::Side(Side::Left)));
        // Mid-drag, before the drop prunes it, the view's group is empty, and so is the dock.
        let view = layout.detach(VIEW, AT);
        assert_eq!(layout.arrange(window, &all).central, window);
        assert!(!layout.dock(view, CENTRAL, Zone::Side(Side::Left)));
        assert!(layout.dock(view, CENTRAL, Zone::Center));
        layout.prune();
        assert!(matches!(&layout.main, Node::Tabs(tabs) if tabs.id == view));
        // Emptied and pruned, the dock is the central area alone, taking the group again.
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
            closed: Vec::new(),
            hidden: Vec::new(),
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
        layout.main = tabs;
        layout.view = None;
        assert!(layout.check().is_err());
    }

    #[test]
    fn a_closed_panel_keeps_its_place_and_the_view_never_closes() {
        let mut layout = Layout::new(Some(VIEW));
        let view = layout.find(VIEW).unwrap();
        let a = layout.float("A".to_owned(), AT);
        assert!(layout.dock(a, view, Zone::Side(Side::Right)));
        let open = layout.clone();
        assert!(layout.close("A") && layout.close("A"));
        assert_eq!(layout.closed, ["A"]);
        assert!(!layout.close(VIEW));
        assert!(!layout.is_closed(VIEW));
        layout.prune();
        assert_eq!(layout.find("A"), Some(a));
        layout.open("A");
        assert_eq!(layout, open);
    }

    #[test]
    fn a_dockspace_showing_no_panel_is_the_empty_dock_which_takes_a_group_beside_the_hidden_ones() {
        let mut layout = Layout::new(Some(VIEW));
        let window = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 600.0));
        let view = layout.find(VIEW).unwrap();
        let a = layout.float("A".to_owned(), AT);
        assert!(layout.dock(a, view, Zone::Side(Side::Right)));
        let view = layout.detach(VIEW, AT);
        layout.prune();
        let hidden = |panel: &str| panel != "A";
        assert_eq!(layout.arrange(window, &hidden).central, window);
        assert!(layout.dock(view, CENTRAL, Zone::Center));
        assert_eq!(layout.check(), Ok(()));
        let arranged = layout.arrange(window, &hidden);
        assert_eq!(arranged.groups, [(view, window)]);
        assert_eq!(arranged.central, Rect::NOTHING);
        assert_eq!(
            layout.arrange(window, &all).groups,
            [
                (a, Rect::from_min_max(pos2(0.0, 0.0), pos2(246.0, 600.0))),
                (
                    view,
                    Rect::from_min_max(pos2(254.0, 0.0), pos2(1000.0, 600.0))
                ),
            ]
        );
    }

    const EDITOR: [(&str, Docked); 3] = [
        ("Inspector", Docked::RightOf(VIEW, 0.25)),
        ("Entities", Docked::LeftOf(VIEW, 0.25)),
        ("Performance", Docked::Below("Inspector", 0.5)),
    ];

    #[test]
    fn panels_dock_as_the_app_says_taking_the_shares_it_gives() {
        let mut layout = Layout::new(Some(VIEW));
        let narrow = [("Inspector", Docked::RightOf(VIEW, 0.2))];
        layout.add("Inspector".to_owned(), AT, &narrow);
        assert!(
            matches!(layout.main, Node::Split { ratio, .. } if ratio == 0.8),
            "{:?}",
            layout.main
        );
        let mut layout = Layout::new(Some(VIEW));
        for (panel, _) in EDITOR {
            layout.add(panel.to_owned(), AT, &EDITOR);
        }
        assert!(layout.floating.is_empty());
        assert_eq!(layout.check(), Ok(()));
        let window = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 600.0));
        let arranged = layout.arrange(window, &all);
        let rect = |panel| {
            let group = layout.find(panel);
            let mut groups = arranged.groups.iter();
            groups.find_map(|&(id, rect)| (Some(id) == group).then_some(rect))
        };
        let at = |min: [f32; 2], max: [f32; 2]| Some(Rect::from_min_max(min.into(), max.into()));
        assert_eq!(rect("Entities"), at([0.0, 0.0], [182.5, 600.0]));
        assert_eq!(rect(VIEW), at([190.5, 0.0], [746.0, 600.0]));
        assert_eq!(rect("Inspector"), at([754.0, 0.0], [1000.0, 296.0]));
        assert_eq!(rect("Performance"), at([754.0, 304.0], [1000.0, 600.0]));
    }

    #[test]
    fn a_target_the_layout_doesnt_hold_stands_for_its_own_and_one_floating_or_none_floats() {
        // Performance, below the absent Inspector, goes below the view, taking a quarter of it,
        // not the half it'd take of the Inspector's place.
        let mut layout = Layout::new(Some(VIEW));
        layout.add("Performance".to_owned(), AT, &EDITOR);
        let Node::Split {
            axis: Axis::Vertical,
            ratio,
            children,
            ..
        } = &layout.main
        else {
            panic!("Performance is below the view: {:?}", layout.main);
        };
        assert_eq!(*ratio, 0.75);
        assert!(matches!(&children[1], Node::Tabs(tabs) if tabs.panels == ["Performance"]));
        let floats = |layout: &Layout, panel| {
            let group = layout.find(panel).expect("the panel is in the layout");
            layout
                .floating(group)
                .is_some_and(|f| f.tabs.panels == [panel])
        };
        let cycle = [("A", Docked::With("B")), ("B", Docked::With("A"))];
        layout.add("A".to_owned(), AT, &cycle);
        assert!(floats(&layout, "A"));
        layout.add("B".to_owned(), AT, &[("B", Docked::Below("Nowhere", 0.5))]);
        assert!(floats(&layout, "B"));
        layout.add("C".to_owned(), AT, &[("C", Docked::With("A"))]);
        assert!(floats(&layout, "C"));
        layout.add("D".to_owned(), AT, &EDITOR);
        assert!(floats(&layout, "D"));
    }

    #[test]
    fn a_panel_docked_among_tabs_leaves_the_one_showing() {
        let mut layout = Layout::new(Some(VIEW));
        layout.add("Log".to_owned(), AT, &[("Log", Docked::With(VIEW))]);
        let group = layout.group(layout.find(VIEW).unwrap()).unwrap();
        assert_eq!(group.panels, [VIEW, "Log"]);
        assert_eq!(group.showing(&all), Some(VIEW));
    }

    /// In the editor's layout with Assets below the view: Entities is left, Inspector and
    /// Performance right, Assets bottom; not the view or a floating group. Hiding a side hides
    /// only its panels. Around a game's central area each strip is on its docked side; a dockspace
    /// whose view floats has no sides.
    #[test]
    fn a_panel_docked_beside_the_view_on_the_way_to_it_is_on_that_side() {
        let mut layout = Layout::new(Some(VIEW));
        let docked = [&EDITOR[..], &[("Assets", Docked::Below(VIEW, 0.25))]].concat();
        for &(panel, _) in &docked {
            layout.add(panel.to_owned(), AT, &docked);
        }
        layout.float("Log".to_owned(), AT);
        let panels = [
            "Entities",
            "Inspector",
            "Performance",
            "Assets",
            VIEW,
            "Log",
            "None",
        ];
        let (left, right) = (Some(Side::Left), Some(Side::Right));
        let (top, bottom) = (Some(Side::Top), Some(Side::Bottom));
        let sides = panels.map(|panel| layout.side(panel));
        assert_eq!(sides, [left, right, right, bottom, None, None, None]);
        layout.set_hidden(Side::Right, true);
        layout.set_hidden(Side::Right, true);
        assert_eq!(layout.hidden, [Side::Right]);
        let hidden = panels.map(|panel| layout.is_hidden(panel));
        assert_eq!(hidden, [false, true, true, false, false, false, false]);
        layout.set_hidden(Side::Right, false);
        layout.set_hidden(Side::Bottom, true);
        let hidden = panels.map(|panel| layout.is_hidden(panel));
        assert_eq!(hidden, [false, false, false, true, false, false, false]);
        layout.set_hidden(Side::Bottom, false);
        assert!(layout.hidden.is_empty());

        let mut game = Layout::new(None);
        let strips = [
            ("A", Side::Left),
            ("B", Side::Bottom),
            ("C", Side::Right),
            ("D", Side::Top),
        ];
        for (panel, side) in strips {
            strip(&mut game, panel, side);
        }
        let sides = ["A", "B", "C", "D"].map(|panel| game.side(panel));
        assert_eq!(sides, [left, bottom, right, top]);

        layout.detach(VIEW, AT);
        layout.prune();
        assert_eq!(layout.side("Entities"), None);
    }
}
