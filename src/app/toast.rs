//! jot's own notices: a card at the bottom right of the window, for a
//! long command that ended out of sight, a trigger, `jot notify`. A
//! click goes to the pane it is about; it goes by itself after a while.
//! When the window is not in front, the desktop is told as well.

use super::*;

pub struct Toast {
    pub title: String,
    pub body: String,
    /// The color of its edge: green done, red failed, the accent else.
    pub color: u32,
    pub pane: Option<PaneId>,
    pub at: Instant,
    pub rect: Rect,
}

const SHOWN: Duration = Duration::from_secs(7);

impl App {
    /// A notice in the window, and on the desktop when the window is not
    /// in front.
    pub(super) fn toast(&mut self, title: &str, body: &str, color: Option<u32>, pane: Option<PaneId>) {
        if !self.focused_window {
            automate::notify(&format!("jot · {title}"), body);
        }
        self.toasts.push(Toast { title: title.into(), body: body.into(), color: color.unwrap_or(self.cur_accent), pane, at: Instant::now(), rect: Rect::default() });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
        self.full = true;
        self.redraw();
    }

    /// Notices that have been shown long enough go.
    pub(super) fn expire_toasts(&mut self) {
        let before = self.toasts.len();
        self.toasts.retain(|t| t.at.elapsed() < SHOWN);
        if self.toasts.len() != before {
            // What they covered comes back with the next frame (floats_off).
            self.redraw();
        }
    }

    /// A click on a notice: to its pane, and the notice gone.
    pub(super) fn toast_click(&mut self) -> bool {
        let (x, y) = self.pointer;
        let Some(i) = self.toasts.iter().position(|t| t.rect.contains(x, y)) else { return false };
        let t = self.toasts.remove(i);
        if let Some(id) = t.pane {
            if let Some(wi) = self.workspaces.iter().position(|w| w.root.contains(id)) {
                self.go_to(wi);
                self.set_focus(id);
            }
        }
        self.full = true;
        self.relayout();
        true
    }

    pub(super) fn draw_toasts(&mut self) {
        if self.toasts.is_empty() {
            return;
        }
        let c = self.cur_theme.chrome();
        let scale = self.scale;
        let px = |v: f32| (v * scale).round() as i32;
        let w = px(380.0).min(self.canvas.width - px(32.0));
        let (uh, sh) = (self.fonts.ui.height, self.fonts.small.height);
        let h = px(14.0) + uh + px(4.0) + sh + px(14.0);
        let mut bottom = self.canvas.height - self.bottom_h() - px(12.0);
        for i in (0..self.toasts.len()).rev() {
            let r = Rect { x: self.canvas.width - w - px(16.0), y: bottom - h, w, h };
            let (title, body, color) = (self.toasts[i].title.clone(), self.toasts[i].body.clone(), self.toasts[i].color);
            self.float_save(Rect { x: r.x - px(16.0), y: r.y - px(16.0), w: r.w + px(32.0), h: r.h + px(32.0) });
            self.canvas.panel(r, px(10.0), c.panel, c.edge);
            self.canvas.round_fill(Rect { x: r.x + px(10.0), y: r.y + px(12.0), w: px(4.0), h: h - px(24.0) }, px(2.0), color, 255);
            let tx = r.x + px(24.0);
            self.canvas.text_fit(&mut self.fonts.ui, &title, tx, r.y + px(12.0), w - px(36.0), c.text, r);
            self.canvas.text_fit(&mut self.fonts.small, &body, tx, r.y + px(16.0) + uh, w - px(36.0), c.dim, r);
            self.toasts[i].rect = r;
            self.damage.push(Rect { x: r.x - px(16.0), y: r.y - px(16.0), w: r.w + px(32.0), h: r.h + px(32.0) });
            bottom = r.y - px(10.0);
        }
    }
}

impl App {
    /// The floating things last drawn (toasts, the tooltip) taken off
    /// again, what was under them put back: at the start of a frame drawn
    /// in part. A frame drawn whole draws under them anyway.
    pub(super) fn floats_off(&mut self, full: bool) {
        let under = std::mem::take(&mut self.floats_under);
        if full {
            return;
        }
        for u in under.iter().rev() {
            self.canvas.restore(u);
            self.damage.push(u.0);
        }
    }

    /// What `r` covers kept, before a floating thing is drawn on it.
    pub(super) fn float_save(&mut self, r: Rect) {
        let u = self.canvas.save(r);
        self.floats_under.push(u);
    }
}
