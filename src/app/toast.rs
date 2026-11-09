//! jot's own notices: a card at the bottom right of the window, for a
//! long command that ended out of sight, a trigger, `jot notify`. Two
//! lines, what happened and its detail, beside a mark (done, something to
//! know, something wrong: a glyph, never a color); at its right what can
//! be done about it, if anything, and an × to send it away. A click
//! elsewhere on it goes to the pane it is about. It goes by itself after a
//! while, but not while the pointer is on it. When the window is not in
//! front, the desktop is told as well.

use super::*;
use crate::palette::Action;

/// What a toast's mark says.
#[derive(Clone, Copy, PartialEq)]
pub enum Mark {
    Done,
    Note,
    Problem,
}

/// What on a toast is under the pointer.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ToastHit {
    Card,
    Action,
    Close,
}

pub struct Toast {
    pub title: String,
    pub body: String,
    pub mark: Mark,
    pub pane: Option<PaneId>,
    /// A button on it, and what it does.
    pub action: Option<(String, Action)>,
    pub at: Instant,
    pub rect: Rect,
    pub act_at: Rect,
    pub close_at: Rect,
    pub lit: Option<ToastHit>,
}

const SHOWN: Duration = Duration::from_secs(7);

/// The red jot has said "failed" with, and the green "done": now the mark.
const RED: u32 = 0xE5484D;
const GREEN: u32 = 0x30A46C;

impl App {
    /// A notice in the window, and on the desktop when the window is not
    /// in front. `color` says what kind it is, as callers have said it:
    /// red something wrong, green done.
    pub(super) fn toast(&mut self, title: &str, body: &str, color: Option<u32>, pane: Option<PaneId>) {
        self.toast_with(title, body, color, pane, None);
    }

    /// A notice with a button on it (Reconnect, for a machine lost).
    pub(super) fn toast_with(&mut self, title: &str, body: &str, color: Option<u32>, pane: Option<PaneId>, action: Option<(&str, Action)>) {
        if !self.focused_window {
            automate::notify(&format!("jot · {title}"), body);
        }
        let mark = match color {
            Some(RED) => Mark::Problem,
            Some(GREEN) => Mark::Done,
            _ if title == "Saved" => Mark::Done,
            _ => Mark::Note,
        };
        let action = action.map(|(l, a)| (l.to_string(), a));
        self.toasts.push(Toast { title: title.into(), body: body.into(), mark, pane, action, at: Instant::now(), rect: Rect::default(), act_at: Rect::default(), close_at: Rect::default(), lit: None });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
        self.redraw();
    }

    /// Notices that have been shown long enough go; not one under the
    /// pointer, which is being read.
    pub(super) fn expire_toasts(&mut self) {
        let before = self.toasts.len();
        self.toasts.retain(|t| t.lit.is_some() || t.at.elapsed() < SHOWN);
        if self.toasts.len() != before {
            // What they covered comes back with the next frame (floats_off).
            self.redraw();
        }
    }

    /// The pointer over the toasts: the button under it lit. True when it
    /// is on one, the pointer then theirs.
    pub(super) fn toast_hover(&mut self, x: i32, y: i32) -> bool {
        let mut on = false;
        let mut changed = false;
        for t in &mut self.toasts {
            let hit = if !t.rect.contains(x, y) {
                None
            } else if t.close_at.contains(x, y) {
                Some(ToastHit::Close)
            } else if t.action.is_some() && t.act_at.contains(x, y) {
                Some(ToastHit::Action)
            } else {
                Some(ToastHit::Card)
            };
            on |= hit.is_some();
            if t.lit != hit {
                // Away from it, its time starts again.
                if hit.is_none() {
                    t.at = Instant::now();
                }
                t.lit = hit;
                changed = true;
            }
        }
        if changed {
            self.redraw();
        }
        if on {
            if let Some(w) = &self.window {
                w.set_cursor(CursorIcon::Pointer);
            }
        }
        on
    }

    /// A click on a notice: its × sends it away, its button does what it
    /// says, anywhere else goes to its pane. The notice goes with each.
    pub(super) fn toast_click(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let (x, y) = self.pointer;
        let Some(i) = self.toasts.iter().position(|t| t.rect.contains(x, y)) else { return false };
        let t = self.toasts.remove(i);
        self.redraw();
        if t.close_at.contains(x, y) {
            return true;
        }
        if let Some((_, action)) = t.action.filter(|_| t.act_at.contains(x, y)) {
            self.run(action, event_loop);
            return true;
        }
        if let Some(id) = t.pane {
            if let Some(wi) = self.workspaces.iter().position(|w| w.root.contains(id)) {
                self.go_to(wi);
                self.set_focus(id);
            }
            self.full = true;
            self.relayout();
        }
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
        let h = px(62.0).max(uh + sh + px(24.0));
        // A quiet lift for the mark's square and the button, a stronger
        // one under the pointer; from the theme, so light themes have them.
        let (lift, lit) = (mix(c.panel, c.text, 16), mix(c.panel, c.text, 30));
        let mut bottom = self.canvas.height - self.bottom_h() - px(12.0);
        for i in (0..self.toasts.len()).rev() {
            let r = Rect { x: self.canvas.width - w - px(16.0), y: bottom - h, w, h };
            let t = &self.toasts[i];
            let (title, body, mark, label, hover) = (t.title.clone(), t.body.clone(), t.mark, t.action.as_ref().map(|a| a.0.clone()), t.lit);
            self.float_save(Rect { x: r.x - px(18.0), y: r.y - px(18.0), w: r.w + px(36.0), h: r.h + px(36.0) });
            self.canvas.panel(r, px(8.0), c.panel, c.edge);
            // The mark, in its square.
            let well = Rect { x: r.x + px(12.0), y: r.y + (h - px(30.0)) / 2, w: px(30.0), h: px(30.0) };
            self.canvas.round_fill(well, px(7.0), lift, 255);
            let glyph = match mark {
                Mark::Done => crate::canvas::icons::CHECK,
                Mark::Note => crate::canvas::icons::INFO,
                Mark::Problem => crate::canvas::icons::DANGER,
            };
            self.canvas.icon_in(glyph, well, px(15.0), c.text);
            // At the right, the × and before it the button.
            let b = px(26.0);
            let close = Rect { x: r.x + r.w - px(10.0) - b, y: r.y + (h - b) / 2, w: b, h: b };
            if hover == Some(ToastHit::Close) {
                self.canvas.round_fill(close, px(6.0), lit, 255);
            }
            let x_color = if hover == Some(ToastHit::Close) { c.text } else { c.dim };
            self.canvas.icon_in(crate::canvas::icons::CLOSE, close, px(12.0), x_color);
            let mut right = close.x - px(4.0);
            let mut act = Rect::default();
            if let Some(label) = &label {
                let bw = self.fonts.ui.width(label) + px(20.0);
                act = Rect { x: right - bw, y: r.y + (h - b) / 2, w: bw, h: b };
                self.canvas.round_fill(act, px(6.0), if hover == Some(ToastHit::Action) { lit } else { lift }, 255);
                self.canvas.text(&mut self.fonts.ui, label, (act.x + px(10.0)) as f32, act.y + (b - uh) / 2, c.text, act);
                right = act.x - px(8.0);
            }
            // The two lines, what happened and its detail.
            let tx = well.x + well.w + px(12.0);
            let room = (right - tx).max(0);
            let top = r.y + (h - uh - px(3.0) - sh) / 2;
            self.canvas.text_fit(&mut self.fonts.ui, &title, tx, top, room, c.text, r);
            self.canvas.text_fit(&mut self.fonts.small, &body, tx, top + uh + px(3.0), room, c.dim, r);
            let t = &mut self.toasts[i];
            t.rect = r;
            t.act_at = act;
            t.close_at = close;
            self.damage.push(Rect { x: r.x - px(18.0), y: r.y - px(18.0), w: r.w + px(36.0), h: r.h + px(36.0) });
            bottom = r.y - px(8.0);
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
