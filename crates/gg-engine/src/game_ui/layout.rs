//! Laying a game's UI out, flexbox-lite, in canvas units: sizes fixed, fit or fill; children in a
//! row, column, list or stack, with padding, gaps and alignment; top nodes placed in the view from
//! their anchors; and what clips each.

use gg_core::math::Vec2;

use super::{Kind, Node, Size, widgets};
use crate::Align;
use crate::font::Fonts;

/// A node's top left from the view's top left (+Y down), and its size, in canvas units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Rect {
    pub(crate) at: Vec2,
    pub(crate) size: Vec2,
}

impl Rect {
    /// Whether `point`, in the same units from the same corner, is in it. Its top and left edges
    /// count, its bottom and right don't, so neighbors share no point.
    pub(crate) fn contains(&self, point: Vec2) -> bool {
        point.cmpge(self.at).all() && point.cmplt(self.at + self.size).all()
    }

    pub(crate) fn middle(&self) -> Vec2 {
        self.at + self.size / 2.0
    }

    /// It less `padding` on each side; no size where the padding takes it all.
    pub(crate) fn inside(&self, padding: Vec2) -> Self {
        Self {
            at: self.at + padding,
            size: (self.size - padding * 2.0).max(Vec2::ZERO),
        }
    }

    /// What of it is in `other` too; no size where none is.
    pub(crate) fn within(&self, other: Rect) -> Self {
        let at = self.at.max(other.at);
        let end = (self.at + self.size).min(other.at + other.size);
        Self {
            at,
            size: (end - at).max(Vec2::ZERO),
        }
    }
}

/// The share of the spare room `align` puts before what it aligns: none, half or all.
pub(crate) fn share(align: Align) -> f32 {
    match align {
        Align::Start => 0.0,
        Align::Center => 0.5,
        Align::End => 1.0,
    }
}

/// The children of node `parent`, in order: each follows the subtree of the one before.
pub(crate) fn children(nodes: &[Node], parent: usize) -> impl Iterator<Item = usize> + '_ {
    let end = nodes[parent].end as usize;
    let first = Some(parent + 1).filter(|&child| child < end);
    std::iter::successors(first, move |&child| {
        Some(nodes[child].end as usize).filter(|&next| next < end)
    })
}

/// What a layout finds, kept between layouts so laying out as many nodes again allocates nothing:
/// each node's fit (the least a fit or fill takes), its rectangle, whether it shows (it and every
/// node above visible), and what it draws within (the rectangle less padding of each clipping list
/// or container above).
#[derive(Default)]
pub(crate) struct Laid {
    pub(crate) fit: Vec<Vec2>,
    /// Where each node draws and is found: its place in the layout moved by its offset and each
    /// offset above it ([`settle`](Self::settle)).
    pub(crate) rects: Vec<Rect>,
    pub(crate) shown: Vec<bool>,
    pub(crate) clips: Vec<Option<Rect>>,
    /// Each node's place in the layout, before offsets.
    base: Vec<Rect>,
    /// Each node's offset and those above it, as [`settle`](Self::settle) summed them.
    shift: Vec<Vec2>,
    /// Which fills a row or column holds at their fit while sharing out its room.
    held: Vec<bool>,
    /// How far a row's, column's or list's children reach along it, gaps and margins included;
    /// a list scrolls through it.
    reach: Vec<f32>,
}

impl Laid {
    /// Moves each node from its place in the layout by its offset and those above it, and finds
    /// what clips each from the rectangles so moved: a clipping node's offset moves its clip, not
    /// the clips of nodes beside it. Allocates nothing once the buffers have grown.
    pub(crate) fn settle(&mut self, nodes: &[Node]) {
        self.shift.clear();
        self.shift.resize(nodes.len(), Vec2::ZERO);
        for (i, node) in nodes.iter().enumerate() {
            let above = node
                .parent
                .map_or(Vec2::ZERO, |parent| self.shift[parent as usize]);
            self.shift[i] = above + node.offset;
            self.rects[i] = Rect {
                at: self.base[i].at + self.shift[i],
                size: self.base[i].size,
            };
        }
        for (i, node) in nodes.iter().enumerate() {
            if !self.shown[i] {
                continue;
            }
            let clip = match node.clips {
                true => {
                    let room = self.rects[i].inside(node.padding);
                    Some(self.clips[i].map_or(room, |clip| room.within(clip)))
                }
                false => self.clips[i],
            };
            for child in children(nodes, i) {
                self.clips[child] = clip;
            }
        }
    }

    /// Whether `point` is within what clips node `i`.
    pub(crate) fn within(&self, i: usize, point: Vec2) -> bool {
        self.clips[i].is_none_or(|clip| clip.contains(point))
    }

    /// How far list `i` scrolls at most: its children's reach past its room.
    pub(crate) fn most_scroll(&self, nodes: &[Node], i: usize) -> f32 {
        let room = self.rects[i].inside(nodes[i].padding).size.y;
        (self.reach[i] - room).max(0.0)
    }
}

/// Lays `nodes` (a UI's, in pre-order) out in a view `view` units across and down, measuring texts
/// with `fonts`, into `laid`.
///
/// Fit, from the children up:
/// - A text's is its text's box; an image's is its texture's size, a unit a pixel.
/// - A container's is its visible children's: side by side in a row or column with `gap`
///   between, or the largest for a stack or panel; plus its padding.
/// - A fixed size replaces the fit.
///
/// Rectangles, from the parent down:
/// - A top node goes in the view; a child goes in its parent less its padding.
/// - Along a row's width or a column's height, the room the fixed and fit children and gaps
///   leave is shared among the fills by their shares, none below its fit. Room left without a
///   fill to take it goes before, around or after the children as the parent's `align` says.
/// - Across a row or column, and both ways in a stack or panel, a fill takes all the room.
/// - A child goes where its `anchor`, else its parent's `align`, puts it.
/// - A fill is never less than its fit; a node larger than its parent's room overflows it.
/// - A margin is room round a node, outside it.
/// - A hidden node and what's under it take no room and have no size, at its parent's top left.
///
/// A list lays its children out as a column does, from its top whatever its alignment down,
/// scrolled up by its scroll within what they reach past its room. One that fills its parent down
/// fits none of them, so it takes the room it's given however much it holds.
///
/// Each node draws within the rectangle less its padding of each clipping list and container
/// above it ([`Laid::settle`] finds them, with the offsets).
pub(crate) fn lay_out(nodes: &[Node], fonts: &mut Fonts, view: Vec2, laid: &mut Laid) {
    let count = nodes.len();
    laid.fit.clear();
    laid.fit.resize(count, Vec2::ZERO);
    laid.rects.clear();
    laid.rects.resize(count, Rect::default());
    laid.shown.clear();
    laid.shown.resize(count, false);
    laid.clips.clear();
    laid.clips.resize(count, None);
    laid.held.clear();
    laid.held.resize(count, false);
    laid.reach.clear();
    laid.reach.resize(count, 0.0);
    for (i, node) in nodes.iter().enumerate() {
        let above = node.parent.is_none_or(|parent| laid.shown[parent as usize]);
        laid.shown[i] = above && node.visible;
    }
    for i in (0..count).rev() {
        laid.fit[i] = fit(nodes, i, fonts, &laid.fit);
    }
    let view = Rect {
        at: Vec2::ZERO,
        size: view,
    };
    for i in 0..count {
        if nodes[i].parent.is_none() {
            laid.rects[i] = match laid.shown[i] {
                true => placed(&nodes[i], laid.fit[i], view, [Align::Center; 2]),
                false => Rect::default(),
            };
        }
        if !laid.shown[i] {
            let at = laid.rects[i].at;
            for child in children(nodes, i) {
                laid.rects[child] = Rect {
                    at,
                    size: Vec2::ZERO,
                };
            }
            continue;
        }
        match nodes[i].kind {
            Kind::Row => line(nodes, i, 0, laid),
            Kind::Column => line(nodes, i, 1, laid),
            Kind::List => {
                line(nodes, i, 1, laid);
                let scroll = nodes[i].scroll.clamp(0.0, laid.most_scroll(nodes, i));
                for child in children(nodes, i) {
                    laid.rects[child].at.y -= scroll;
                }
            }
            Kind::Panel | Kind::Stack => stack(nodes, i, laid),
            Kind::Text
            | Kind::Image
            | Kind::Button
            | Kind::Slider
            | Kind::Toggle
            | Kind::Dropdown
            | Kind::Field => {}
        }
    }
    laid.base.clone_from(&laid.rects);
}

/// The size node `i` fits to, its children's already in `fits` ([`lay_out`]).
fn fit(nodes: &[Node], i: usize, fonts: &mut Fonts, fits: &[Vec2]) -> Vec2 {
    let node = &nodes[i];
    if !node.visible {
        return Vec2::ZERO;
    }
    // A fixed size skips measuring content, such as a text's shaping.
    if let [Size::Fixed(width), Size::Fixed(height)] = node.size {
        return Vec2::new(width, height);
    }
    let outside = |child: usize| fits[child] + nodes[child].margin * 2.0;
    let visible = children(nodes, i).filter(|&child| nodes[child].visible);
    let content = match node.kind {
        Kind::Text => fonts.measure(&node.text, &node.style(node.looks[0].text)),
        Kind::Button => {
            fonts.measure(&node.text, &node.style(node.looks[0].text)) + node.padding * 2.0
        }
        Kind::Image => node.natural.unwrap_or(Vec2::ZERO),
        Kind::Slider | Kind::Toggle | Kind::Dropdown | Kind::Field => widgets::fit(node, fonts),
        // A list that fills its parent down takes the room it's given and scrolls the rest.
        Kind::List if matches!(node.size[1], Size::Fill(_)) => {
            let across = visible.map(|child| outside(child).x).fold(0.0, f32::max);
            Vec2::new(across, 0.0) + node.padding * 2.0
        }
        Kind::Row | Kind::Column | Kind::List => {
            let axis = usize::from(node.kind != Kind::Row);
            let (mut along, mut across, mut count) = (0.0, 0.0_f32, 0_usize);
            for child in visible {
                let size = outside(child);
                along += size[axis];
                across = across.max(size[1 - axis]);
                count += 1;
            }
            along += node.gap * count.saturating_sub(1) as f32;
            let mut content = Vec2::ZERO;
            content[axis] = along;
            content[1 - axis] = across;
            content + node.padding * 2.0
        }
        Kind::Panel | Kind::Stack => {
            visible.map(outside).fold(Vec2::ZERO, Vec2::max) + node.padding * 2.0
        }
    };
    let mut fit = content;
    for axis in 0..2 {
        if let Size::Fixed(units) = node.size[axis] {
            fit[axis] = units;
        }
    }
    // An image fit on one axis keeps its texture's shape from the other, fixed one.
    if let Some(natural) = node
        .natural
        .filter(|natural| natural.cmpgt(Vec2::ZERO).all())
    {
        match node.size {
            [Size::Fixed(width), Size::Fit] => fit.y = width * natural.y / natural.x,
            [Size::Fit, Size::Fixed(height)] => fit.x = height * natural.x / natural.y,
            _ => {}
        }
    }
    fit
}

/// Where `node`, of fit `fit`, goes in `room` (a stack's or panel's less its padding, or the
/// view). A fill takes all of it less its margin, but not less than its fit. It goes where its
/// anchor, else `align`, puts it, overflowing the room round that point where it's larger.
fn placed(node: &Node, fit: Vec2, room: Rect, align: [Align; 2]) -> Rect {
    let anchor = node.anchor.unwrap_or(align);
    let room = room.inside(node.margin);
    let mut rect = Rect::default();
    for axis in 0..2 {
        let size = match node.size[axis] {
            Size::Fill(_) => room.size[axis].max(fit[axis]),
            Size::Fixed(_) | Size::Fit => fit[axis],
        };
        rect.size[axis] = size;
        rect.at[axis] = room.at[axis] + (room.size[axis] - size) * share(anchor[axis]);
    }
    rect
}

/// Places the children of stack or panel `i` ([`lay_out`]).
fn stack(nodes: &[Node], i: usize, laid: &mut Laid) {
    let room = laid.rects[i].inside(nodes[i].padding);
    for child in children(nodes, i) {
        laid.rects[child] = match nodes[child].visible {
            true => placed(&nodes[child], laid.fit[child], room, nodes[i].align),
            false => Rect {
                at: room.at,
                size: Vec2::ZERO,
            },
        };
    }
}

/// Places the children of row or column `i` along `axis`, 0 across and 1 down ([`lay_out`]).
fn line(nodes: &[Node], i: usize, axis: usize, laid: &mut Laid) {
    let node = &nodes[i];
    let room = laid.rects[i].inside(node.padding);
    let across = 1 - axis;
    let (mut used, mut shares, mut count) = (0.0, 0.0, 0_usize);
    for child in children(nodes, i).filter(|&child| nodes[child].visible) {
        count += 1;
        used += nodes[child].margin[axis] * 2.0;
        match nodes[child].size[axis] {
            Size::Fill(share) => shares += share,
            Size::Fixed(_) | Size::Fit => used += laid.fit[child][axis],
        }
    }
    used += node.gap * count.saturating_sub(1) as f32;
    // Share what's left among the fills; one whose share is below its fit is held at its fit, and
    // the rest is shared again among the others.
    let mut left = room.size[axis] - used;
    let mut each;
    loop {
        each = if shares > 0.0 {
            left.max(0.0) / shares
        } else {
            0.0
        };
        let mut held = false;
        for child in children(nodes, i) {
            let fit = laid.fit[child][axis];
            if let Size::Fill(share) = nodes[child].size[axis]
                && nodes[child].visible
                && !laid.held[child]
                && each * share < fit
            {
                laid.held[child] = true;
                left -= fit;
                shares -= share;
                held = true;
            }
        }
        if !held {
            break;
        }
    }
    let along = |child: usize, laid: &Laid| match nodes[child].size[axis] {
        Size::Fill(share) if !laid.held[child] => each * share,
        Size::Fill(_) | Size::Fixed(_) | Size::Fit => laid.fit[child][axis],
    };
    let mut total = node.gap * count.saturating_sub(1) as f32;
    for child in children(nodes, i).filter(|&child| nodes[child].visible) {
        total += along(child, laid) + nodes[child].margin[axis] * 2.0;
    }
    laid.reach[i] = total;
    // A list's children start at its top, for scrolling.
    let lead = match node.kind {
        Kind::List => 0.0,
        _ => share(node.align[axis]),
    };
    let mut at = room.at[axis] + (room.size[axis] - total) * lead;
    for child in children(nodes, i) {
        let kid = &nodes[child];
        if !kid.visible {
            laid.rects[child] = Rect {
                at: room.at,
                size: Vec2::ZERO,
            };
            continue;
        }
        let size = along(child, laid);
        let anchor = kid
            .anchor
            .map_or(node.align[across], |anchor| anchor[across]);
        let reach = (room.size[across] - kid.margin[across] * 2.0).max(0.0);
        let thick = match kid.size[across] {
            Size::Fill(_) => reach.max(laid.fit[child][across]),
            Size::Fixed(_) | Size::Fit => laid.fit[child][across],
        };
        let mut rect = Rect::default();
        rect.at[axis] = at + kid.margin[axis];
        rect.size[axis] = size;
        rect.at[across] = room.at[across] + kid.margin[across] + (reach - thick) * share(anchor);
        rect.size[across] = thick;
        laid.rects[child] = rect;
        at += size + kid.margin[axis] * 2.0 + node.gap;
    }
}
