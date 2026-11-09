//! Laying a game's UI out, flexbox-lite, in the canvas's units: each node sized fixed, fit to what
//! it holds or filling its share of its parent, its children in a row, a column, a list or
//! stacked, with padding, gaps between them and alignment both ways, a list's scrolled; the nodes
//! at its top in the view, each from the point of it that its anchor names; and what clips each.

use gg_core::math::Vec2;

use super::{Kind, Node, Size, widgets};
use crate::Align;
use crate::font::Fonts;

/// Where a node is: its top left, from the view's top left, +Y down, and its size, in the canvas's
/// units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Rect {
    pub(crate) at: Vec2,
    pub(crate) size: Vec2,
}

impl Rect {
    /// Whether `point`, in the same units from the same corner, is in it: on its top or left edge,
    /// but not on its bottom or right, so that two side by side share no point.
    pub(crate) fn contains(&self, point: Vec2) -> bool {
        point.cmpge(self.at).all() && point.cmplt(self.at + self.size).all()
    }

    pub(crate) fn middle(&self) -> Vec2 {
        self.at + self.size / 2.0
    }

    /// It without `padding` on each side, of no size where the padding takes it all.
    pub(crate) fn inside(&self, padding: Vec2) -> Self {
        Self {
            at: self.at + padding,
            size: (self.size - padding * 2.0).max(Vec2::ZERO),
        }
    }

    /// What of it is in `other` too, of no size where none is.
    pub(crate) fn within(&self, other: Rect) -> Self {
        let at = self.at.max(other.at);
        let end = (self.at + self.size).min(other.at + other.size);
        Self {
            at,
            size: (end - at).max(Vec2::ZERO),
        }
    }
}

/// How far along an axis `align` puts what it aligns, of the room there is: none at its start,
/// half at its middle, all at its end.
pub(crate) fn share(align: Align) -> f32 {
    match align {
        Align::Start => 0.0,
        Align::Center => 0.5,
        Align::End => 1.0,
    }
}

/// The children of `nodes`' node `parent`, in order, as their pre-order lays them out: each after
/// the subtree of the one before.
pub(crate) fn children(nodes: &[Node], parent: usize) -> impl Iterator<Item = usize> + '_ {
    let end = nodes[parent].end as usize;
    let first = Some(parent + 1).filter(|&child| child < end);
    std::iter::successors(first, move |&child| {
        Some(nodes[child].end as usize).filter(|&next| next < end)
    })
}

/// What a layout finds, kept from one to the next, so that laying out as many nodes again
/// allocates nothing: each node's size fit to what it holds, the size a fit or a fill takes at
/// least; where each is; whether each shows, it and each node above it visible; and what each
/// draws within, the rectangle less its padding of each list and container that clips above it.
#[derive(Default)]
pub(crate) struct Laid {
    pub(crate) fit: Vec<Vec2>,
    pub(crate) rects: Vec<Rect>,
    pub(crate) shown: Vec<bool>,
    pub(crate) clips: Vec<Option<Rect>>,
    /// Which fills a row or a column holds at their fit, as it shares out what room it has.
    held: Vec<bool>,
    /// How far a row's, a column's or a list's children reach along it, their gaps and margins
    /// among them, which a list scrolls through.
    reach: Vec<f32>,
}

impl Laid {
    /// Whether `point` is within what clips node `i`.
    pub(crate) fn within(&self, i: usize, point: Vec2) -> bool {
        self.clips[i].is_none_or(|clip| clip.contains(point))
    }

    /// How far list `i` of `nodes` scrolls at most: as far as its children reach past its room.
    pub(crate) fn most_scroll(&self, nodes: &[Node], i: usize) -> f32 {
        let room = self.rects[i].inside(nodes[i].padding).size.y;
        (self.reach[i] - room).max(0.0)
    }
}

/// Lays `nodes`, a UI's in pre-order, out in a view `view` units across and down, its texts
/// measured by `fonts`, into `laid`: first each node's fit, from its children up, a text's its
/// text's box, an image's its texture's size, a unit a pixel, and a container's its visible
/// children's, side by side in a row or a column, `gap` between each two, or the largest for a
/// stack or a panel, with its padding round them; a fixed size in place of a fit. Then each node's
/// rectangle, from its parent down, a node at the top in the view, and each child in its parent
/// less its padding: in a row along the parent's width, in a column along its height, the room
/// the fixed and fit children and the gaps leave shared among the fills by their shares, none
/// below its fit, and what's left over, without a fill to take it, put before, around or after
/// them as the parent's `align` says; across them, and in a stack or a panel both ways, a fill
/// takes all the room, and a child goes where its `anchor`, or else its parent's `align`, puts it.
/// A fill is never less than its fit, either way, and a node larger than its parent's room
/// overflows it. A list lays its children out as a column does, from its top, whatever its
/// alignment down, scrolled up by its scroll, within what they reach past its room; one that
/// fills its parent down fits none of them, so that it takes the room it's given, however much it
/// holds. Each node draws within the rectangle less its padding of each list and container that
/// clips above it.
/// A margin is room round a node, outside it. A hidden node, and what's under it, takes no room,
/// and is of no size at its parent's top left.
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
        let clip = match nodes[i].clips {
            true => {
                let room = laid.rects[i].inside(nodes[i].padding);
                Some(laid.clips[i].map_or(room, |clip| room.within(clip)))
            }
            false => laid.clips[i],
        };
        for child in children(nodes, i) {
            laid.clips[child] = clip;
        }
    }
}

/// The size node `i` of `nodes` fits to, its children's already in `fits`, as [`lay_out`] says.
fn fit(nodes: &[Node], i: usize, fonts: &mut Fonts, fits: &[Vec2]) -> Vec2 {
    let node = &nodes[i];
    if !node.visible {
        return Vec2::ZERO;
    }
    // Without measuring what it holds, as a text's shaping, which a fixed size takes the place of.
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
        // A list that fills its parent down takes what room it's given, and scrolls the rest.
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
    // An image keeps its texture's shape along the axis it fits on, where the other is fixed.
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

/// Where `node`, of fit `fit`, goes in `room`, a stack's or a panel's less its padding, or the
/// view: a fill takes all of it, less its margin, but none less than its fit, and it goes where its
/// anchor, or else `align`, puts it, overflowing the room round that point where it's larger.
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

/// Places the children of stack or panel `i` of `nodes`, as [`lay_out`] says.
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

/// Places the children of row or column `i` of `nodes` along `axis`, 0 across and 1 down, as
/// [`lay_out`] says.
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
    // What's left, shared out among the fills, each held at its fit while its share is less, which
    // shares out the rest again among the others.
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
    // A list's children start at its top, as it scrolls them.
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
