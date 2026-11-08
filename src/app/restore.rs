//! Opening fresh, with the last session offered back, as Chrome does: a
//! small card at the top right says what it held, with Restore. A click on
//! Restore brings it back; typing anything starts fresh and the card goes.
//! Until one or the other, the last session is left as it was saved.

use super::*;
use crate::canvas::on;

pub struct Offer {
    pub session: Session,
    pub summary: String,
    pub restore: Rect,
    pub close: Rect,
    pub hover: Option<bool>,
}

impl App {
    /// The last session, held to be offered rather than brought back.
    pub(super) fn offer(&mut self, session: Session) {
        let ws = session.workspaces.len();
        let panes: usize = session
            .workspaces
            .iter()
            .map(|w| {
                fn count(s: &persist::Spec) -> usize {
                    match s {
                        persist::Spec::Across { v, .. } => v.iter().map(count).sum(),
                        persist::Spec::Down { h, .. } => h.iter().map(count).sum(),
                        _ => 1,
                    }
                }
                w.layout.as_ref().map_or(w.panes.len().max(1), count)
            })
            .sum::<usize>()
            + session.popouts.len();
        let ago = persist::session_saved_at().and_then(|t| t.elapsed().ok()).map(|d| format!(" · {} ago", commands::human(d))).unwrap_or_default();
        let s = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let summary = format!("{} · {}{ago}", s(ws, "workspace", "workspaces"), s(panes, "pane", "panes"));
        self.offered = Some(Offer { session, summary, restore: Rect::default(), close: Rect::default(), hover: None });
    }

    /// The card goes, and what is open now is the session from here on.
    pub(super) fn dismiss_offer(&mut self) {
        if self.offered.take().is_some() {
            self.unsaved = true;
            self.full = true;
            self.redraw();
        }
    }

    /// The last session back, in place of what was opened fresh.
    fn restore_offered(&mut self, event_loop: &ActiveEventLoop) {
        let Some(offer) = self.offered.take() else { return };
        self.find = None;
        self.copy_mode = None;
        self.editors.clear();
        self.popouts.clear();
        self.focused_popout = None;
        self.workspaces.clear();
        self.roam = None;
        self.roam_shown = false;
        for p in self.panes.values_mut() {
            p.pty.end = true;
        }
        self.panes.clear();
        self.session = Some(offer.session);
        self.start();
        self.open_pending_popouts(event_loop);
        self.set_title();
        self.unsaved = true;
        self.changed();
    }

    /// A click on the card: Restore, or close. True when it was on it.
    pub(super) fn offer_click(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let (x, y) = self.pointer;
        let Some(o) = &self.offered else { return false };
        if o.restore.contains(x, y) {
            self.restore_offered(event_loop);
            true
        } else if o.close.contains(x, y) {
            self.dismiss_offer();
            true
        } else {
            false
        }
    }

    pub(super) fn offer_hover(&mut self, x: i32, y: i32) -> bool {
        let Some(o) = &mut self.offered else { return false };
        let hit = if o.restore.contains(x, y) { Some(true) } else if o.close.contains(x, y) { Some(false) } else { None };
        if o.hover != hit {
            o.hover = hit;
            self.full = true;
            self.redraw();
        }
        if hit.is_some() {
            if let Some(w) = &self.window {
                w.set_cursor(CursorIcon::Pointer);
            }
        }
        hit.is_some()
    }

    pub(super) fn draw_offer(&mut self) {
        let Some(o) = &self.offered else { return };
        let (summary, hover) = (o.summary.clone(), o.hover);
        let c = self.cur_theme.chrome();
        let accent = self.cur_accent;
        let scale = self.scale;
        let px = |v: f32| (v * scale).round() as i32;
        let (uh, sh) = (self.fonts.ui.height, self.fonts.small.height);
        let w = px(330.0).min(self.canvas.width - px(32.0));
        let h = px(14.0) + uh + px(4.0) + sh + px(14.0) + uh + px(12.0) + px(14.0);
        let r = Rect { x: self.canvas.width - w - px(16.0), y: px(16.0), w, h };
        self.canvas.panel(r, px(10.0), c.panel, c.edge);
        let tx = r.x + px(16.0);
        self.canvas.text(&mut self.fonts.ui, "Restore last session?", tx as f32, r.y + px(14.0), c.text, r);
        self.canvas.text_fit(&mut self.fonts.small, &summary, tx, r.y + px(18.0) + uh, w - px(32.0), c.dim, r);
        let bh = uh + px(12.0);
        let by = r.y + h - px(14.0) - bh;
        let bw = self.fonts.ui.width("Restore") + px(28.0);
        let restore = Rect { x: tx, y: by, w: bw, h: bh };
        self.canvas.round_fill(restore, px(7.0), if hover == Some(true) { mix(accent, 0xFFFFFF, 30) } else { accent }, 255);
        self.canvas.text(&mut self.fonts.ui, "Restore", (restore.x + px(14.0)) as f32, restore.y + px(6.0), on(accent), restore);
        // Close, an × at the top right.
        let cs = px(24.0);
        let close = Rect { x: r.x + r.w - cs - px(8.0), y: r.y + px(8.0), w: cs, h: cs };
        if hover == Some(false) {
            self.canvas.round_fill(close, cs / 2, c.hover, 255);
        }
        self.canvas.icon_in(crate::canvas::icons::CLOSE, close, px(14.0), c.dim);
        if let Some(o) = &mut self.offered {
            o.restore = restore;
            o.close = close;
        }
        self.damage.push(Rect { x: r.x - px(16.0), y: 0, w: r.w + px(32.0), h: r.y + r.h + px(16.0) });
    }
}

impl App {
    /// A last session of more than one window, brought back as it was: this
    /// window takes up the first's workspaces, where it was and as large;
    /// each other opens again, a jot of its own (--restore) with its own.
    pub(super) fn split_windows(&mut self, mut s: Session) -> Session {
        let mut windows = std::mem::take(&mut s.windows);
        if windows.len() < 2 || crate::quake_mode() || crate::restoring() {
            return s;
        }
        let mine = windows.remove(0);
        let dir = persist::state_dir();
        for (i, w) in windows.iter().enumerate() {
            let path = dir.join(format!("restore-{}-{i}.toml", std::process::id()));
            let Ok(text) = toml::to_string(w) else { continue };
            if std::fs::write(&path, text).is_err() {
                continue;
            }
            // Opened once this window has said its workspaces to the session:
            // the shared ones are there for it then (see App::save).
            self.open_later.push(path);
        }
        if let Some(st) = mine.window {
            self.start_window = Some(st);
            self.normal = Some((st.x, st.y, st.w, st.h));
            if (st.font_size - self.font_size).abs() > 0.01 {
                self.set_font_size(st.font_size);
            }
            // Open already (the session was offered): moved to where it was.
            if let Some(w) = &self.window {
                w.set_maximized(false);
                let _ = w.request_inner_size(winit::dpi::PhysicalSize::new(st.w, st.h));
                w.set_outer_position(winit::dpi::PhysicalPosition::new(st.x, st.y));
                w.set_maximized(st.maximized);
            }
        }
        mine
    }
}
