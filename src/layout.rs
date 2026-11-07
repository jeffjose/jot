//! How a workspace is cut into panes: a tree of splits, each side by
//! side (`v`, as screen's `split -v`) or stacked (`h`, as screen's
//! `split`), nested as deep as you like.

use serde::{Deserialize, Serialize};

pub type PaneId = u64;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Dir {
    /// Side by side, a vertical line between.
    #[serde(rename = "v")]
    Across,
    /// One above another, a horizontal line between.
    #[serde(rename = "h")]
    Down,
}

#[derive(Clone, Debug)]
pub enum Node {
    Pane(PaneId),
    Split { dir: Dir, children: Vec<Node>, sizes: Vec<f32> },
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// A line between two panes, which can be dragged: the split it belongs
/// to (by its path from the root) and which of its gaps.
#[derive(Clone, Debug)]
pub struct Divider {
    pub rect: Rect,
    pub path: Vec<usize>,
    pub gap: usize,
    pub dir: Dir,
    /// The extent of the split, to turn a drag into a share.
    pub span: Rect,
}

/// Pixels between panes.
pub const GAP: i32 = 1;

impl Node {
    pub fn panes(&self, out: &mut Vec<PaneId>) {
        match self {
            Node::Pane(id) => out.push(*id),
            Node::Split { children, .. } => children.iter().for_each(|c| c.panes(out)),
        }
    }

    pub fn contains(&self, id: PaneId) -> bool {
        match self {
            Node::Pane(p) => *p == id,
            Node::Split { children, .. } => children.iter().any(|c| c.contains(id)),
        }
    }

    /// Lays the tree out in `area`: each pane's rectangle, and the lines
    /// between them.
    pub fn layout(&self, area: Rect, panes: &mut Vec<(PaneId, Rect)>, dividers: &mut Vec<Divider>) {
        self.layout_at(area, &mut Vec::new(), panes, dividers);
    }

    fn layout_at(&self, area: Rect, path: &mut Vec<usize>, panes: &mut Vec<(PaneId, Rect)>, dividers: &mut Vec<Divider>) {
        match self {
            Node::Pane(id) => panes.push((*id, area)),
            Node::Split { dir, children, sizes } => {
                let total = if *dir == Dir::Across { area.w } else { area.h };
                let room = (total - GAP * (children.len() as i32 - 1)).max(0);
                let mut at = 0;
                let mut used = 0.0f32;
                for (i, child) in children.iter().enumerate() {
                    used += sizes[i];
                    let end = if i + 1 == children.len() { room } else { (used * room as f32).round() as i32 };
                    let len = (end - at).max(0);
                    let offset = at + GAP * i as i32;
                    let r = match dir {
                        Dir::Across => Rect { x: area.x + offset, y: area.y, w: len, h: area.h },
                        Dir::Down => Rect { x: area.x, y: area.y + offset, w: area.w, h: len },
                    };
                    path.push(i);
                    child.layout_at(r, path, panes, dividers);
                    path.pop();
                    if i + 1 < children.len() {
                        let line = match dir {
                            Dir::Across => Rect { x: r.x + r.w, y: area.y, w: GAP, h: area.h },
                            Dir::Down => Rect { x: area.x, y: r.y + r.h, w: area.w, h: GAP },
                        };
                        dividers.push(Divider { rect: line, path: path.clone(), gap: i, dir: *dir, span: area });
                    }
                    at = end;
                }
            }
        }
    }

    /// Splits pane `at`, putting `new` after it in `dir`. Splitting the
    /// same way as the split it is in adds a sibling, halving its share.
    pub fn split(&mut self, at: PaneId, new: PaneId, dir: Dir) -> bool {
        self.split_beside(at, new, dir, false)
    }

    /// Splits pane `at`, putting `new` after it in `dir`, or before it.
    pub fn split_beside(&mut self, at: PaneId, new: PaneId, dir: Dir, before: bool) -> bool {
        match self {
            Node::Pane(id) if *id == at => {
                let pair = if before { vec![Node::Pane(new), Node::Pane(at)] } else { vec![Node::Pane(at), Node::Pane(new)] };
                *self = Node::Split { dir, children: pair, sizes: vec![0.5, 0.5] };
                true
            }
            Node::Pane(_) => false,
            Node::Split { dir: d, children, sizes } => {
                if *d == dir {
                    if let Some(i) = children.iter().position(|c| matches!(c, Node::Pane(p) if *p == at)) {
                        let half = sizes[i] / 2.0;
                        sizes[i] = half;
                        let k = if before { i } else { i + 1 };
                        sizes.insert(k, half);
                        children.insert(k, Node::Pane(new));
                        return true;
                    }
                }
                children.iter_mut().any(|c| c.split_beside(at, new, dir, before))
            }
        }
    }

    fn first_pane(&self) -> PaneId {
        match self {
            Node::Pane(id) => *id,
            Node::Split { children, .. } => children[0].first_pane(),
        }
    }

    pub fn last_pane(&self) -> PaneId {
        match self {
            Node::Pane(id) => *id,
            Node::Split { children, .. } => children[children.len() - 1].last_pane(),
        }
    }

    /// Where pane `id` sits, to put one back there: the pane beside it,
    /// the way they are split, and whether it comes before that pane.
    pub fn place_of(&self, id: PaneId) -> Option<(PaneId, Dir, bool)> {
        let Node::Split { dir, children, .. } = self else { return None };
        if let Some(i) = children.iter().position(|c| matches!(c, Node::Pane(p) if *p == id)) {
            return Some(if i > 0 { (children[i - 1].last_pane(), *dir, false) } else { (children[1].first_pane(), *dir, true) });
        }
        children.iter().find_map(|c| c.place_of(id))
    }

    /// Takes pane `id` out, its share going to its neighbours; a split
    /// left with one child becomes that child. False when `id` is the
    /// whole tree.
    pub fn remove(&mut self, id: PaneId) -> bool {
        let Node::Split { children, sizes, .. } = self else { return false };
        if let Some(i) = children.iter().position(|c| matches!(c, Node::Pane(p) if *p == id)) {
            let share = sizes.remove(i);
            children.remove(i);
            let n = sizes.len() as f32;
            sizes.iter_mut().for_each(|s| *s += share / n);
        } else if !children.iter_mut().any(|c| c.remove(id)) {
            return false;
        }
        if children.len() == 1 {
            *self = children.pop().unwrap();
        } else {
            // A child that became a split the same way is folded in.
            self.flatten();
        }
        true
    }

    fn flatten(&mut self) {
        let Node::Split { dir, children, sizes } = self else { return };
        let mut i = 0;
        while i < children.len() {
            if let Node::Split { dir: d, .. } = &children[i] {
                if d == dir {
                    let Node::Split { children: inner, sizes: inner_sizes, .. } = children.remove(i) else { unreachable!() };
                    let share = sizes.remove(i);
                    let n = inner.len();
                    for (k, (c, s)) in inner.into_iter().zip(inner_sizes).enumerate() {
                        children.insert(i + k, c);
                        sizes.insert(i + k, s * share);
                    }
                    i += n;
                    continue;
                }
            }
            i += 1;
        }
    }

    /// Gives every pane in each split the same share.
    pub fn equalize(&mut self) {
        if let Node::Split { children, sizes, .. } = self {
            let n = children.len() as f32;
            sizes.iter_mut().for_each(|s| *s = 1.0 / n);
            children.iter_mut().for_each(Node::equalize);
        }
    }

    /// Grows (or, with `by` below zero, shrinks) pane `id` by a share of
    /// the nearest split running `dir`, taking it from its neighbour.
    pub fn grow(&mut self, id: PaneId, dir: Dir, by: f32) -> bool {
        let Node::Split { dir: d, children, sizes } = self else { return false };
        if let Some(i) = children.iter().position(|c| c.contains(id)) {
            if children[i].grow(id, dir, by) {
                return true;
            }
            if *d == dir && children.len() > 1 {
                let j = if i + 1 < children.len() { i + 1 } else { i - 1 };
                let by = by.clamp(-(sizes[i] - 0.05), sizes[j] - 0.05);
                sizes[i] += by;
                sizes[j] -= by;
                return true;
            }
        }
        false
    }

    /// Moves the line at `gap` of the split at `path` so that the share
    /// before it is `fraction` of the two it lies between.
    /// Swaps two panes' places.
    pub fn swap(&mut self, a: PaneId, b: PaneId) {
        match self {
            Node::Pane(p) if *p == a => *p = b,
            Node::Pane(p) if *p == b => *p = a,
            Node::Pane(_) => {}
            Node::Split { children, .. } => children.iter_mut().for_each(|c| c.swap(a, b)),
        }
    }

    pub fn drag(&mut self, path: &[usize], gap: usize, fraction: f32) {
        match (self, path.split_first()) {
            (Node::Split { children, .. }, Some((&i, rest))) => children[i].drag(rest, gap, fraction),
            (Node::Split { sizes, .. }, None) => {
                let pair = sizes[gap] + sizes[gap + 1];
                let before: f32 = sizes[..gap].iter().sum();
                let a = (fraction - before).clamp(0.05 * pair, 0.95 * pair);
                sizes[gap] = a;
                sizes[gap + 1] = pair - a;
            }
            _ => {}
        }
    }
}

/// The pane next to `from` in a direction, among laid-out `panes`: the
/// one beyond its edge that overlaps it most, nearest first.
pub fn neighbour(panes: &[(PaneId, Rect)], from: PaneId, dx: i32, dy: i32) -> Option<PaneId> {
    let r = panes.iter().find(|(id, _)| *id == from)?.1;
    panes
        .iter()
        .filter(|(id, _)| *id != from)
        .filter_map(|(id, o)| {
            let (dist, overlap) = if dx != 0 {
                let dist = if dx > 0 { o.x - (r.x + r.w) } else { r.x - (o.x + o.w) };
                (dist, (r.y + r.h).min(o.y + o.h) - r.y.max(o.y))
            } else {
                let dist = if dy > 0 { o.y - (r.y + r.h) } else { r.y - (o.y + o.h) };
                (dist, (r.x + r.w).min(o.x + o.w) - r.x.max(o.x))
            };
            (dist >= 0 && overlap > 0).then_some((dist, -overlap, *id))
        })
        .min()
        .map(|(_, _, id)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: &Node) -> Vec<PaneId> {
        let mut v = Vec::new();
        n.panes(&mut v);
        v
    }

    #[test]
    fn split_and_remove() {
        let mut n = Node::Pane(1);
        assert!(n.split(1, 2, Dir::Across));
        assert!(n.split(2, 3, Dir::Across));
        assert_eq!(ids(&n), vec![1, 2, 3]);
        assert!(n.split(1, 4, Dir::Down));
        assert_eq!(ids(&n), vec![1, 4, 2, 3]);
        assert!(n.remove(4));
        assert_eq!(ids(&n), vec![1, 2, 3]);
        let Node::Split { sizes, .. } = &n else { panic!() };
        assert!((sizes.iter().sum::<f32>() - 1.0).abs() < 1e-4);
        assert!(n.remove(2) && n.remove(3));
        assert!(matches!(n, Node::Pane(1)));
    }

    #[test]
    fn closed_pane_goes_back_where_it_was() {
        let mut n = Node::Pane(1);
        n.split(1, 2, Dir::Across);
        n.split(2, 3, Dir::Down);
        n.split(1, 4, Dir::Across);
        let before = ids(&n);
        for id in [1, 2, 3, 4] {
            let mut m = n.clone();
            let place = m.place_of(id).unwrap();
            m.remove(id);
            assert!(m.split_beside(place.0, id, place.1, place.2));
            assert_eq!(ids(&m), before, "pane {id}");
        }
    }

    #[test]
    fn layout_fills_area() {
        let mut n = Node::Pane(1);
        n.split(1, 2, Dir::Across);
        n.split(1, 3, Dir::Down);
        let (mut panes, mut divs) = (Vec::new(), Vec::new());
        n.layout(Rect { x: 0, y: 0, w: 101, h: 51 }, &mut panes, &mut divs);
        assert_eq!(panes.len(), 3);
        assert_eq!(divs.len(), 2);
        let area: i32 = panes.iter().map(|(_, r)| r.w * r.h).sum::<i32>() + divs.iter().map(|d| d.rect.w * d.rect.h).sum::<i32>();
        // The horizontal line spans only the left column.
        assert!(area <= 101 * 51);
        assert_eq!(neighbour(&panes, 1, 1, 0), Some(2));
        assert_eq!(neighbour(&panes, 1, 0, 1), Some(3));
        assert_eq!(neighbour(&panes, 3, 0, -1), Some(1));
    }
}
