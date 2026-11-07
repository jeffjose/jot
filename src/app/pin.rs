//! Pinned commands: commands kept at the top of a pane, a dense row each,
//! ▶ to run one at the prompt (or queue it while the pane is busy), × to
//! unpin. Three rows show; more scroll. Pins are saved with the session.

use super::*;

pub struct Pin {
    pub cmd: String,
    /// When it last ended here, and how.
    pub last: Option<(Instant, Option<i32>)>,
}

/// What can be clicked on a pin's row.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PinHit {
    Run,
    Remove,
    Row,
}

const SHOWN: usize = 3;

impl App {
    fn pin_row_h(&self) -> i32 {
        self.fonts.ui.height + self.px(6.0)
    }

    /// The height of a pane's pin strip: up to three rows, then it scrolls.
    pub(super) fn pins_height(&self, id: PaneId) -> i32 {
        match self.panes.get(&id).map_or(0, |p| p.pins.len()) {
            0 => 0,
            n => n.min(SHOWN) as i32 * self.pin_row_h() + self.px(4.0),
        }
    }

    /// A command pinned to a pane, once.
    pub(super) fn pin_add(&mut self, id: PaneId, cmd: &str) {
        let cmd = cmd.trim();
        if cmd.is_empty() {
            return;
        }
        let Some(p) = self.panes.get_mut(&id) else { return };
        if !p.pins.iter().any(|pin| pin.cmd == cmd) {
            p.pins.push(Pin { cmd: cmd.to_string(), last: None });
            // The newest in view.
            p.pins_scroll = p.pins.len().saturating_sub(SHOWN);
        }
        self.unsaved = true;
        self.relayout();
    }

    pub(super) fn pin_remove(&mut self, id: PaneId, i: usize) {
        let Some(p) = self.panes.get_mut(&id) else { return };
        if i < p.pins.len() {
            p.pins.remove(i);
        }
        p.pins_scroll = p.pins_scroll.min(p.pins.len().saturating_sub(SHOWN));
        self.unsaved = true;
        self.relayout();
    }

    pub(super) fn pin_run(&mut self, id: PaneId, i: usize) {
        let Some(cmd) = self.panes.get(&id).and_then(|p| p.pins.get(i)).map(|pin| pin.cmd.clone()) else { return };
        self.set_focus(id);
        self.run_or_queue(id, cmd);
    }

    /// A command ended in a pane: the pins that are it remember how.
    pub(super) fn pins_ended(p: &mut Pane, cmd: &str, exit: Option<i32>) {
        for pin in p.pins.iter_mut().filter(|pin| pin.cmd == cmd.trim()) {
            pin.last = Some((Instant::now(), exit));
        }
    }

    /// The wheel over a pin strip: it scrolls. True when it was over one.
    pub(super) fn pins_wheel(&mut self, lines: f32) -> bool {
        let (x, y) = self.pointer;
        let Some(id) = self.panes.iter().find(|(id, p)| {
            let h = self.pins_height(**id);
            h > 0 && Rect { x: p.rect.x, y: p.rect.y, w: p.rect.w, h }.contains(x, y)
        }).map(|(id, _)| *id) else {
            return false;
        };
        let Some(p) = self.panes.get_mut(&id) else { return false };
        let most = p.pins.len().saturating_sub(SHOWN);
        // Up the wheel (lines < 0), to the earlier pins.
        p.pins_scroll = if lines < 0.0 { p.pins_scroll.saturating_sub(1) } else { (p.pins_scroll + 1).min(most) };
        p.full = true;
        self.full = true;
        self.redraw();
        true
    }

    /// A pane's pin strip, along its top.
    pub(super) fn draw_pins(&mut self, id: PaneId) {
        let h = self.pins_height(id);
        self.pin_hits.retain(|(_, p, _, _)| *p != id);
        if h == 0 {
            return;
        }
        let Some(p) = self.panes.get(&id) else { return };
        let area = Rect { x: p.rect.x, y: p.rect.y, w: p.rect.w, h };
        let running = p.shell.running.as_ref().filter(|_| !p.shell.at_prompt).map(|r| r.2.trim().to_string());
        let busy = p.shell.marked && !p.shell.at_prompt;
        let rows: Vec<(usize, String, Option<(Instant, Option<i32>)>)> = p.pins.iter().enumerate().skip(p.pins_scroll).take(SHOWN).map(|(i, pin)| (i, pin.cmd.clone(), pin.last)).collect();
        let (total, scroll) = (p.pins.len(), p.pins_scroll);
        let c = self.cur_theme.chrome();
        let accent = self.cur_accent;
        let scale = self.scale;
        let px = |v: f32| (v * scale).round() as i32;
        let (sh, mh) = (self.fonts.small.height, self.fonts.mono.height);
        let row_h = self.pin_row_h();
        self.canvas.fill(area, c.side);
        self.canvas.fill(Rect { y: area.y + area.h - 1, h: 1, ..area }, c.edge);
        let ib = row_h - px(4.0);
        let mut y = area.y + px(2.0);
        for (i, cmd, last) in rows {
            let row = Rect { x: area.x, y, w: area.w, h: row_h };
            let lit = |hit| self.pin_lit == Some((id, i, hit));
            if lit(PinHit::Row) || lit(PinHit::Run) || lit(PinHit::Remove) {
                self.canvas.fill(row, c.hover);
            }
            // ▶ at the left: run, or queue while the pane is busy.
            let play = Rect { x: area.x + px(16.0), y: y + (row_h - ib) / 2, w: ib, h: ib };
            if lit(PinHit::Run) {
                self.canvas.round_fill(play, px(4.0), mix(c.hover, c.text, 30), 255);
            }
            let (cx, cy, s) = (play.x + ib / 2 - px(1.0), play.y + ib / 2, px(4.0));
            let tri = if lit(PinHit::Run) { accent } else { c.dim };
            for t in 0..=s * 2 {
                let half = (s * 2 - t) / 2;
                self.canvas.fill(Rect { x: cx - s + t, y: cy - half, w: 1, h: half * 2 + 1 }, tri);
            }
            // The command, then how it last went, then × while the pointer is
            // on the row.
            let x = play.x + ib + px(8.0);
            let x_btn = Rect { x: area.x + area.w - px(6.0) - ib, y: y + (row_h - ib) / 2, w: ib, h: ib };
            let meta = if running.as_deref() == Some(cmd.as_str()) {
                ("running".to_string(), Some(0x30A46C))
            } else if busy {
                ("queues".to_string(), None)
            } else {
                match last {
                    Some((at, exit)) => (ago(at.elapsed()), Some(if exit.is_some_and(|e| e != 0) { 0xE5484D } else { 0x30A46C })),
                    None => (String::new(), None),
                }
            };
            let mw = self.fonts.small.width(&meta.0);
            let meta_x = x_btn.x - px(10.0) - mw;
            if let Some(dot) = meta.1 {
                let d = px(6.0);
                self.canvas.round_fill(Rect { x: meta_x - d - px(6.0), y: y + (row_h - d) / 2, w: d, h: d }, d / 2, dot, 255);
            }
            self.canvas.text(&mut self.fonts.small, &meta.0, meta_x as f32, y + (row_h - sh) / 2, c.faint, area);
            self.canvas.text_fit(&mut self.fonts.mono, &cmd, x, y + (row_h - mh) / 2, (meta_x - px(20.0) - x).max(0), c.text, area);
            if lit(PinHit::Row) || lit(PinHit::Run) || lit(PinHit::Remove) {
                if lit(PinHit::Remove) {
                    self.canvas.round_fill(x_btn, px(4.0), mix(c.hover, c.text, 30), 255);
                }
                let (bx, by, arm, dot) = (x_btn.x + ib / 2, x_btn.y + ib / 2, px(3.5), px(1.5).max(2));
                let color = if lit(PinHit::Remove) { 0xE5484D } else { c.dim };
                for t in -arm..=arm {
                    self.canvas.round_fill(Rect { x: bx + t - dot / 2, y: by + t - dot / 2, w: dot, h: dot }, dot / 2, color, 255);
                    self.canvas.round_fill(Rect { x: bx + t - dot / 2, y: by - t - dot / 2, w: dot, h: dot }, dot / 2, color, 255);
                }
            }
            self.pin_hits.push((play, id, i, PinHit::Run));
            self.pin_hits.push((x_btn, id, i, PinHit::Remove));
            self.pin_hits.push((row, id, i, PinHit::Row));
            y += row_h;
        }
        // More than three: a thin bar at the right says where the view is.
        if total > SHOWN {
            let track = area.h - px(4.0);
            let th = (track * SHOWN as i32 / total as i32).max(px(8.0));
            let ty = area.y + px(2.0) + (track - th) * scroll as i32 / (total - SHOWN) as i32;
            self.canvas.round_fill(Rect { x: area.x + area.w - px(3.0), y: ty, w: px(2.0).max(2), h: th }, px(1.0), c.faint, 255);
        }
        self.damage.push(area);
    }

    /// A click on a pin strip: ▶ runs, × unpins. True when it was on one.
    pub(super) fn pin_click(&mut self) -> bool {
        let (x, y) = self.pointer;
        let Some((_, id, i, hit)) = self.pin_hits.iter().find(|(r, _, _, h)| r.contains(x, y) && *h != PinHit::Row).or_else(|| self.pin_hits.iter().find(|(r, _, _, _)| r.contains(x, y))).copied() else { return false };
        match hit {
            PinHit::Run => self.pin_run(id, i),
            PinHit::Remove => self.pin_remove(id, i),
            PinHit::Row => self.set_focus(id),
        }
        true
    }

    /// The pointer over a pin strip: its row lit, the hand on its buttons.
    pub(super) fn pin_hover(&mut self, x: i32, y: i32) -> bool {
        let at = self.pin_hits.iter().find(|(r, _, _, h)| r.contains(x, y) && *h != PinHit::Row).or_else(|| self.pin_hits.iter().find(|(r, _, _, _)| r.contains(x, y))).map(|(_, id, i, h)| (*id, *i, *h));
        if at != self.pin_lit {
            for id in [self.pin_lit.map(|l| l.0), at.map(|a| a.0)].into_iter().flatten() {
                if let Some(p) = self.panes.get_mut(&id) {
                    p.full = true;
                }
            }
            self.pin_lit = at;
            self.redraw();
        }
        if let Some((_, _, hit)) = at {
            if let Some(w) = &self.window {
                w.set_cursor(if hit == PinHit::Row { CursorIcon::Default } else { CursorIcon::Pointer });
            }
        }
        at.is_some()
    }

    /// A right click on a pin: what can be done with it.
    pub(super) fn pin_menu(&mut self) -> bool {
        let (x, y) = self.pointer;
        let Some((_, id, i, _)) = self.pin_hits.iter().find(|(r, _, _, _)| r.contains(x, y)).copied() else { return false };
        use crate::palette::{Action, Item};
        let c = |label: &str, hint: &str, action| Some(Item { label: label.into(), hint: hint.into(), action });
        self.open_items_menu(vec![c("Run", "", Action::PinRun(id, i)), c("Edit command", "", Action::PinEdit(id, i)), None, c("Unpin", "", Action::PinRemove(id, i))]);
        true
    }
}

/// How long ago, to the nearest unit worth saying.
fn ago(d: Duration) -> String {
    match d.as_secs() {
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < 86400 => format!("{}h ago", s / 3600),
        s => format!("{}d ago", s / 86400),
    }
}
