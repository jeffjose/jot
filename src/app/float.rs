//! A pane's focus and minimize buttons, floating at its top right while the
//! pointer is over it, as VS Code's editor actions do: on a small panel of
//! their own, each lit under the pointer.

use super::*;

impl App {
    /// The pointer has moved: the pane it is over shows its buttons, and
    /// the one it left stops.
    pub(super) fn float_track(&mut self, x: i32, y: i32) {
        let over = if self.drag.is_some() || self.roam_shown { None } else { self.pane_at(x, y).map(|(id, _)| id) };
        // On the icons, or off them: the panel comes and goes.
        let near = self.float_rect.filter(|(id, _)| Some(*id) == over).map(|(_, r)| r.contains(x, y));
        if near != self.float_near {
            self.float_near = near;
            if let Some(id) = over {
                self.float_refresh(id);
                self.redraw();
            }
        }
        if over == self.float_pane {
            return;
        }
        for id in [self.float_pane, over].into_iter().flatten() {
            self.float_refresh(id);
        }
        self.float_pane = over;
        self.redraw();
    }

    /// The pointer has left the window.
    pub(super) fn float_leave(&mut self) {
        if let Some(id) = self.float_pane.take() {
            self.float_refresh(id);
            self.redraw();
        }
    }

    /// A pane drawn again whole, and all of it put up: the room above its
    /// first line, where the panel was, is not otherwise.
    fn float_refresh(&mut self, id: PaneId) {
        if let Some(p) = self.panes.get_mut(&id) {
            p.full = true;
            let r = p.rect;
            self.damage.push(r);
        }
    }

    /// The buttons over a pane, when the pointer is on it: minimize and
    /// focus; focused, only the way back. At rest just the two icons, out
    /// of the way; with the pointer on them, their panel and the one lit.
    pub(super) fn draw_float(&mut self, id: PaneId) {
        self.multi_tags.retain(|(_, p, k)| !(*p == id && matches!(k, watch::Tag::Focus | watch::Tag::Minimize)));
        if self.float_pane != Some(id) || self.roam_shown || !self.ws().root.contains(id) || self.ws().minimized.contains(&id) {
            return;
        }
        let zoomed = self.ws().zoom.is_some();
        let mut ids = Vec::new();
        self.ws().root.panes(&mut ids);
        if ids.len() < 2 && !zoomed {
            return;
        }
        let Some((rect, bg)) = self.panes.get(&id).map(|p| (p.rect, p.term.colors().1)) else { return };
        let kinds: &[watch::Tag] = if zoomed { &[watch::Tag::Focus] } else { &[watch::Tag::Minimize, watch::Tag::Focus] };
        let c = self.cur_theme.chrome();
        let (b, gap, inset, radius) = (self.px(24.0), self.px(2.0), self.px(3.0), self.px(7.0));
        let w = kinds.len() as i32 * b + (kinds.len() as i32 - 1) * gap + 2 * inset;
        let h = b + 2 * inset;
        // Clear of the pane's edge by more than the panel's shadow spreads.
        let margin = self.px(12.0);
        let panel = Rect { x: rect.x + rect.w - margin - w, y: rect.y + self.pins_height(id) + self.px(8.0), w, h };
        if panel.x < rect.x || panel.y + h > rect.y + rect.h {
            return;
        }
        self.float_rect = Some((id, panel));
        let near = panel.contains(self.pointer.0, self.pointer.1);
        if near {
            self.canvas.panel(panel, radius, c.side, c.edge);
        } else {
            // Text under the icons kept from crowding them: the pane's own
            // background, which shows as nothing.
            self.canvas.round_fill(panel, radius, bg, 255);
        }
        let mut x = panel.x + inset;
        for kind in kinds {
            let r = Rect { x, y: panel.y + inset, w: b, h: b };
            let lit = self.multi_lit == Some((id, *kind));
            if lit {
                self.canvas.round_fill(r, radius - inset / 2, c.text, 36);
            }
            let icon = match kind {
                watch::Tag::Minimize => crate::canvas::icons::MINUS,
                _ if zoomed => crate::canvas::icons::UNFOCUS,
                _ => crate::canvas::icons::FOCUS,
            };
            let color = if lit { c.text } else if near { c.dim } else { c.faint };
            self.canvas.icon_in(icon, r, self.px(15.0), color);
            self.multi_tags.push((r, id, *kind));
            x += b + gap;
        }
        let s = margin;
        self.damage.push(Rect { x: panel.x - s, y: panel.y - s, w: panel.w + 2 * s, h: panel.h + 2 * s });
    }
}
