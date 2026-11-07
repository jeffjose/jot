//! Panes moved by dragging, as VS Code's are: take one by its caption and
//! drop it on another pane, on one of its sides to split it there, or in
//! its middle to swap the two.

use super::*;

/// Where on a pane a dragged one would land.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Zone {
    Left,
    Right,
    Top,
    Bottom,
    Swap,
}

/// A pane being dragged.
#[derive(Clone, Copy, Debug)]
pub struct Carry {
    pub pane: PaneId,
    pub from: (i32, i32),
    /// Moved far enough to be a drag rather than a click.
    pub moving: bool,
    /// The pane under the pointer and where on it.
    pub over: Option<(PaneId, Zone)>,
}

/// Which part of `r` a point is in: the outer quarter on each side, else
/// the middle.
pub fn zone(r: Rect, x: i32, y: i32) -> Zone {
    let (fx, fy) = ((x - r.x) as f32 / r.w.max(1) as f32, (y - r.y) as f32 / r.h.max(1) as f32);
    let edges = [(fx, Zone::Left), (1.0 - fx, Zone::Right), (fy, Zone::Top), (1.0 - fy, Zone::Bottom)];
    let (d, z) = edges.iter().copied().fold((f32::MAX, Zone::Swap), |a, b| if b.0 < a.0 { b } else { a });
    if d < 0.25 { z } else { Zone::Swap }
}

/// The part of `r` a pane dropped in `z` would take.
pub fn zone_rect(r: Rect, z: Zone) -> Rect {
    match z {
        Zone::Left => Rect { w: r.w / 2, ..r },
        Zone::Right => Rect { x: r.x + r.w / 2, w: r.w - r.w / 2, ..r },
        Zone::Top => Rect { h: r.h / 2, ..r },
        Zone::Bottom => Rect { y: r.y + r.h / 2, h: r.h - r.h / 2, ..r },
        Zone::Swap => r,
    }
}

impl App {
    /// The side of a pane's corner handle: a triangle at its top left, in
    /// the room left around the text, so no text is ever under it.
    pub(super) fn handle_size(&self) -> i32 {
        self.px(10.0)
    }

    /// Whether panes show their handle: not with one alone on screen.
    fn handles_shown(&self) -> bool {
        if self.roam_shown || self.ws().zoom.is_some() {
            return false;
        }
        let mut ids = Vec::new();
        self.ws().root.panes(&mut ids);
        ids.len() > 1
    }

    /// The pane whose corner handle is at a point, a little more than the
    /// triangle counting, so it is easy to take.
    pub(super) fn handle_at(&self, x: i32, y: i32) -> Option<PaneId> {
        if !self.handles_shown() {
            return None;
        }
        let reach = self.handle_size() + self.px(6.0);
        let mut ids = Vec::new();
        self.ws().root.panes(&mut ids);
        ids.retain(|p| !self.ws().spaces.contains(p) && !self.ws().minimized.contains(p));
        ids.into_iter().find(|id| {
            self.panes.get(id).is_some_and(|p| {
                let (dx, dy) = (x - p.rect.x, y - p.rect.y);
                dx >= 0 && dy >= 0 && dx + dy < reach
            })
        })
    }

    /// The pointer over a handle or off it: the handle lit, and the hand.
    /// True when it is over one.
    pub(super) fn handle_hover(&mut self, x: i32, y: i32) -> bool {
        let at = if self.drag.is_none() { self.handle_at(x, y) } else { None };
        if at != self.handle_hover {
            for id in [self.handle_hover, at].into_iter().flatten() {
                if let Some(p) = self.panes.get_mut(&id) {
                    p.full = true;
                }
            }
            self.handle_hover = at;
            self.redraw();
        }
        if at.is_some() {
            if let Some(w) = &self.window {
                w.set_cursor(CursorIcon::Grab);
            }
        }
        at.is_some()
    }

    /// A pane's handle: gray, in the accent when the pointer is on it.
    pub(super) fn draw_handle(&mut self, id: PaneId) {
        if !self.handles_shown() || !self.ws().root.contains(id) {
            return;
        }
        let Some(r) = self.panes.get(&id).map(|p| p.rect) else { return };
        let c = self.cur_theme.chrome();
        let color = if self.handle_hover == Some(id) { self.cur_accent } else { c.faint };
        let n = self.handle_size();
        for row in 0..n {
            // Solid up to the diagonal, and the pixel on it half, for a
            // smooth edge.
            self.canvas.fill(Rect { x: r.x, y: r.y + row, w: n - row - 1, h: 1 }, color);
            self.canvas.round_fill(Rect { x: r.x + n - row - 1, y: r.y + row, w: 1, h: 1 }, 0, color, 128);
        }
        self.damage.push(Rect { x: r.x, y: r.y, w: n, h: n });
    }

    /// The pane whose caption is at a point, to start a drag from.
    pub(super) fn caption_at(&self, x: i32, y: i32) -> Option<PaneId> {
        if self.roam_shown || !self.config.captions || self.ws().zoom.is_some() {
            return None;
        }
        let cap = self.caption_h();
        let mut ids = Vec::new();
        self.ws().root.panes(&mut ids);
        ids.into_iter().find(|id| self.panes.get(id).is_some_and(|p| Rect { x: p.rect.x, y: p.rect.y + p.rect.h - cap, w: p.rect.w, h: cap }.contains(x, y)))
    }

    pub(super) fn carry_move(&mut self, x: i32, y: i32) {
        let Some(Drag::Pane(mut c)) = self.drag else { return };
        if !c.moving && ((x - c.from.0).abs() + (y - c.from.1).abs()) < self.px(6.0) {
            return;
        }
        if !c.moving {
            // A picture of the pane, taken as the drag starts, to carry.
            if let Some(r) = self.panes.get(&c.pane).map(|p| p.rect) {
                self.ghost = Some(self.canvas.snapshot(r, self.px(260.0)));
            }
        }
        c.moving = true;
        c.over = self.pane_at(x, y).filter(|(id, _)| *id != c.pane).and_then(|(id, _)| self.panes.get(&id).map(|p| (id, zone(p.rect, x, y))));
        self.drag = Some(Drag::Pane(c));
        if let Some(w) = &self.window {
            w.set_cursor(CursorIcon::Grabbing);
        }
        self.full = true;
        self.redraw();
    }

    /// The drop: the pane moved beside the one under it, or swapped with it.
    pub(super) fn carry_drop(&mut self, c: Carry) {
        // Whatever handle was lit, the pointer is somewhere else now.
        self.handle_hover = None;
        if let Some(w) = &self.window {
            w.set_cursor(CursorIcon::Default);
        }
        let Some((target, z)) = c.over.filter(|_| c.moving) else {
            // A click on a caption, not a drag: the pane is the one you are in.
            self.set_focus(c.pane);
            return;
        };
        let ws = self.ws_mut();
        match z {
            Zone::Swap => ws.root.swap(c.pane, target),
            _ => {
                if !ws.root.remove(c.pane) {
                    return;
                }
                let (dir, before) = match z {
                    Zone::Left => (Dir::Across, true),
                    Zone::Right => (Dir::Across, false),
                    Zone::Top => (Dir::Down, true),
                    _ => (Dir::Down, false),
                };
                ws.root.split_beside(target, c.pane, dir, before);
            }
        }
        ws.focus = c.pane;
        if let Some(w) = &self.window {
            w.set_cursor(CursorIcon::Default);
        }
        self.changed();
    }

    /// Where the dragged pane would land, shaded in the accent.
    pub(super) fn draw_carry(&mut self) {
        let Some(Drag::Pane(c)) = self.drag else { return };
        if !c.moving {
            return;
        }
        if let Some(r) = self.panes.get(&c.pane).map(|p| p.rect) {
            // The pane being moved is dimmed where it is.
            self.canvas.round_fill(r, 0, 0x000000, 90);
        }
        if let Some((target, z)) = c.over {
            if let Some(r) = self.panes.get(&target).map(|p| p.rect) {
                let r = zone_rect(r, z);
                let m = self.px(4.0);
                let inner = Rect { x: r.x + m, y: r.y + m, w: r.w - 2 * m, h: r.h - 2 * m };
                self.canvas.round_fill(inner, self.px(8.0), self.cur_accent, 70);
                self.canvas.ring(inner, self.px(8.0), self.px(2.0), self.cur_accent);
            }
        }
        // The ghost: the pane's picture under the pointer, see-through.
        if let Some((pixels, w, h)) = &self.ghost {
            // Kept inside the window: beside the pointer, or above it near the bottom.
            let x = (self.pointer.0 + self.px(14.0)).min(self.canvas.width - w - self.px(8.0));
            let below = self.pointer.1 + self.px(14.0);
            let y = if below + h + self.px(8.0) > self.canvas.height { self.pointer.1 - h - self.px(14.0) } else { below };
            let r = Rect { x, y, w: *w, h: *h };
            self.canvas.panel(Rect { x: x - 1, y: y - 1, w: w + 2, h: h + 2 }, self.px(6.0), 0x000000, self.cur_accent);
            self.canvas.blit(pixels, *w, *h, x, y, 190);
            self.canvas.ring(Rect { x: r.x - 1, y: r.y - 1, w: r.w + 2, h: r.h + 2 }, self.px(6.0), self.px(2.0), self.cur_accent);
        }
        self.damage.push(Rect { x: 0, y: 0, w: self.canvas.width, h: self.canvas.height });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zones() {
        let r = Rect { x: 0, y: 0, w: 100, h: 100 };
        assert_eq!(zone(r, 5, 50), Zone::Left);
        assert_eq!(zone(r, 95, 50), Zone::Right);
        assert_eq!(zone(r, 50, 5), Zone::Top);
        assert_eq!(zone(r, 50, 95), Zone::Bottom);
        assert_eq!(zone(r, 50, 50), Zone::Swap);
    }
}
