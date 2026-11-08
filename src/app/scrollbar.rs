//! A pane's scrollbar: a thin pill floating over its right edge, taking no
//! room of its own (nothing moves when it comes or goes). It is there only
//! when the pane has more than it shows, and only while you scroll (a
//! moment after), point at the edge, or drag it; it widens under the
//! pointer. Dragged, it scrolls the pane; a click above or below it, there.

use super::*;

/// How long it stays after the pane last scrolled.
const LINGER: Duration = Duration::from_millis(1200);

/// A pane's scrollbar, as last drawn.
#[derive(Default)]
pub(super) struct Bar {
    /// Where the view was, and when it last moved.
    offset: u64,
    moved: Option<Instant>,
    /// The pill as drawn, to be drawn over when it goes.
    shown: Option<Rect>,
    /// Whether the view was at the bottom, following what is printed.
    bottom: bool,
}

/// Where a pane's pill is: its track (the edge it runs along) and itself.
struct Geometry {
    track: Rect,
    thumb: Rect,
    /// Rows the view can be scrolled through, and how many it shows.
    span: u64,
}

impl App {
    /// Thin, and a little wider under the pointer or held.
    fn scroll_width(&self, near: bool) -> i32 {
        self.px(if near { 7.0 } else { 4.0 })
    }

    /// The strip along a pane's right edge the pointer brings its bar up in.
    fn scroll_reach(&self) -> i32 {
        self.px(14.0)
    }

    /// Where pane `id`'s pill is, if it has more than it shows.
    fn scroll_geometry(&self, id: PaneId) -> Option<Geometry> {
        let p = self.panes.get(&id)?;
        let (total, offset, len) = p.term.scrollbar();
        if total <= len || len == 0 {
            return None;
        }
        let gap = self.px(3.0);
        let h = (p.rows as i32 * self.fonts.cell_h).min(p.rect.y + p.rect.h - p.content.y).max(1);
        let reach = self.scroll_reach();
        let track = Rect { x: p.rect.x + p.rect.w - reach, y: p.content.y + gap, w: reach, h: (h - 2 * gap).max(1) };
        let thumb_h = ((track.h as f64 * len as f64 / total as f64).round() as i32).max(self.px(24.0)).min(track.h);
        let span = total - len;
        let y = track.y + ((track.h - thumb_h) as f64 * offset.min(span) as f64 / span as f64).round() as i32;
        Some(Geometry { track, thumb: Rect { x: track.x, y, w: track.w, h: thumb_h }, span })
    }

    /// Whether pane `id`'s bar is up: held, pointed at, or the pane
    /// scrolled a moment ago.
    fn scroll_up(&self, id: PaneId) -> bool {
        let held = matches!(self.drag, Some(Drag::Scroll { pane, .. }) if pane == id);
        let near = self.scroll_geometry(id).is_some_and(|g| g.track.contains(self.pointer.0, self.pointer.1));
        let recent = self.scrollbars.get(&id).and_then(|b| b.moved).is_some_and(|t| t.elapsed() < LINGER);
        held || near || recent
    }

    /// Pane `id`'s pill drawn over it, after its text (see draw_pane); one
    /// drawn before that is no longer up, the pane drawn whole again to
    /// take it away.
    pub(super) fn draw_scroll_bar(&mut self, id: PaneId) {
        let (total, offset, len) = self.panes.get(&id).map_or((0, 0, 0), |p| p.term.scrollbar());
        let bottom = offset + len >= total;
        let bar = self.scrollbars.entry(id).or_default();
        // Scrolled (not the view following what is printed at the bottom).
        if bar.offset != offset && !(bottom && bar.bottom) {
            bar.moved = Some(Instant::now());
        }
        bar.offset = offset;
        bar.bottom = bottom;
        let geometry = self.scroll_geometry(id).filter(|_| self.scroll_up(id));
        let Some(g) = geometry else {
            if self.scrollbars.get_mut(&id).and_then(|b| b.shown.take()).is_some() {
                if let Some(p) = self.panes.get_mut(&id) {
                    p.full = true;
                }
                self.redraw();
            }
            return;
        };
        let near = matches!(self.drag, Some(Drag::Scroll { pane, .. }) if pane == id) || g.track.contains(self.pointer.0, self.pointer.1);
        let w = self.scroll_width(near);
        let pill = Rect { x: g.thumb.x + g.thumb.w - w - self.px(3.0), y: g.thumb.y, w, h: g.thumb.h };
        // Moved since it was drawn: the pane whole again, the old one gone.
        let was = self.scrollbars.get(&id).and_then(|b| b.shown);
        if was.is_some_and(|r| r != pill) {
            if let Some(b) = self.scrollbars.get_mut(&id) {
                b.shown = None;
            }
            if let Some(p) = self.panes.get_mut(&id) {
                p.full = true;
            }
            return self.redraw();
        }
        let fg = self.cur_theme.fg;
        self.canvas.round_fill(pill, w / 2, fg, if near { 150 } else { 80 });
        self.damage.push(pill);
        if let Some(b) = self.scrollbars.get_mut(&id) {
            b.shown = Some(pill);
        }
    }

    /// Bars that are to go (the pane stopped scrolling a while ago, the
    /// pointer gone): their panes drawn again. Some time from now when one
    /// is to go later.
    pub(super) fn scrollbars_tick(&mut self) -> Option<Instant> {
        let ids: Vec<PaneId> = self.scrollbars.iter().filter(|(_, b)| b.shown.is_some()).map(|(id, _)| *id).collect();
        let mut next: Option<Instant> = None;
        for id in ids {
            if !self.panes.contains_key(&id) {
                self.scrollbars.remove(&id);
                continue;
            }
            if self.scroll_up(id) {
                if let Some(t) = self.scrollbars.get(&id).and_then(|b| b.moved) {
                    let at = t + LINGER;
                    next = Some(next.map_or(at, |n| n.min(at)));
                }
            } else if let Some(p) = self.panes.get_mut(&id) {
                p.full = true;
                self.redraw();
            }
        }
        next
    }

    /// The pointer moved: a pane's bar it came to or left drawn again.
    pub(super) fn scroll_hover(&mut self, x: i32, y: i32) -> bool {
        let over = self.panes.keys().copied().find(|id| self.scroll_geometry(*id).is_some_and(|g| g.track.contains(x, y)) && self.visible_pane(*id));
        if over != self.scroll_lit {
            for id in [self.scroll_lit, over].into_iter().flatten() {
                if let Some(p) = self.panes.get_mut(&id) {
                    p.full = true;
                }
            }
            self.scroll_lit = over;
            self.redraw();
        }
        if over.is_some() {
            if let Some(w) = &self.window {
                w.set_cursor(CursorIcon::Default);
            }
        }
        over.is_some()
    }

    /// Whether pane `id` is drawn now: in the workspace shown (and the one
    /// zoomed, when one is), or the roaming terminal shown.
    fn visible_pane(&self, id: PaneId) -> bool {
        if self.roam_shown {
            return self.roam == Some(id);
        }
        match self.ws().zoom {
            Some(z) => z == id,
            None => self.ws().root.contains(id),
        }
    }

    /// A press at the edge of a pane with more than it shows: on the pill,
    /// held to drag; above or below it, the pane scrolled there, and held.
    pub(super) fn scroll_press(&mut self) -> bool {
        let (x, y) = self.pointer;
        let Some(id) = self.scroll_lit.filter(|id| self.visible_pane(*id)) else { return false };
        let Some(g) = self.scroll_geometry(id) else { return false };
        if !g.track.contains(x, y) {
            return false;
        }
        let grab = if (g.thumb.y..g.thumb.y + g.thumb.h).contains(&y) { y - g.thumb.y } else { g.thumb.h / 2 };
        self.drag = Some(Drag::Scroll { pane: id, grab });
        self.scroll_drag(y);
        true
    }

    /// The pill dragged to `y`: the pane scrolled as far.
    pub(super) fn scroll_drag(&mut self, y: i32) {
        let Some(Drag::Scroll { pane, grab }) = self.drag else { return };
        let Some(g) = self.scroll_geometry(pane) else { return };
        let room = (g.track.h - g.thumb.h).max(1);
        let at = ((y - grab - g.track.y) as f64 / room as f64).clamp(0.0, 1.0);
        let row = (at * g.span as f64).round() as u64;
        if let Some(p) = self.panes.get_mut(&pane) {
            p.term.scroll_to(row);
            p.full = true;
        }
        self.redraw();
    }

    /// Let go of the pill.
    pub(super) fn scroll_release(&mut self) -> bool {
        let Some(Drag::Scroll { pane, .. }) = self.drag else { return false };
        self.drag = None;
        if let Some(b) = self.scrollbars.get_mut(&pane) {
            b.moved = Some(Instant::now());
        }
        if let Some(p) = self.panes.get_mut(&pane) {
            p.full = true;
        }
        self.redraw();
        true
    }
}
