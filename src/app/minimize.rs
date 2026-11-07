//! Minimized panes: a pane you need not watch, shrunk to a row where it
//! is in the layout. It keeps running; the row shows its number, what it
//! runs, a dot for how it is doing, how long it has gone, and the last
//! line it printed. A click brings it back to its size.

use super::*;

impl App {
    /// The height of a minimized pane's row.
    pub(super) fn mini_row_h(&self) -> i32 {
        self.fonts.ui.height + self.px(10.0)
    }

    /// Whether a pane is minimized in some workspace.
    pub(super) fn is_minimized(&self, id: PaneId) -> bool {
        self.workspaces.iter().any(|w| w.minimized.contains(&id))
    }

    /// A pane shrunk to a row, kept running. The last pane showing in a
    /// workspace stays as it is.
    pub(super) fn minimize(&mut self, id: PaneId) {
        let Some(wi) = self.workspaces.iter().position(|w| w.root.contains(id)) else { return };
        let ws = &mut self.workspaces[wi];
        let mut ids = Vec::new();
        ws.root.panes(&mut ids);
        let showing: Vec<PaneId> = ids.iter().copied().filter(|p| *p != id && !ws.minimized.contains(p)).collect();
        if showing.is_empty() {
            return self.toast("One pane left", "The last pane showing stays", None, Some(id));
        }
        if !ws.minimized.contains(&id) {
            ws.minimized.push(id);
        }
        ws.zoom = None;
        if ws.focus == id {
            ws.focus = showing[0];
        }
        self.changed();
    }

    /// A minimized pane its size again, and the one typed into.
    pub(super) fn restore(&mut self, id: PaneId) {
        let Some(wi) = self.workspaces.iter().position(|w| w.minimized.contains(&id)) else { return };
        let ws = &mut self.workspaces[wi];
        ws.minimized.retain(|p| *p != id);
        ws.zoom = None;
        ws.focus = id;
        if wi != self.current {
            self.go_to(wi);
        }
        self.changed();
    }

    /// A pane gone (closed, its program ended): out of the minimized too;
    /// and if only minimized panes are left, they come back.
    pub(super) fn forget_minimized(&mut self, id: PaneId) {
        for ws in &mut self.workspaces {
            ws.minimized.retain(|m| *m != id);
            let mut ids = Vec::new();
            ws.root.panes(&mut ids);
            if ids.iter().filter(|p| **p != id).all(|p| ws.minimized.contains(p)) {
                ws.minimized.clear();
            }
        }
    }

    /// The last line a pane printed, as plain text, for its row.
    pub(super) fn note_last_line(&mut self, id: PaneId, bytes: &[u8]) {
        let text = automate::plain(bytes);
        let Some(p) = self.panes.get_mut(&id) else { return };
        if let Some(line) = text.lines().rev().map(str::trim_end).find(|l| !l.trim().is_empty()) {
            p.last_line = line.trim_start().to_string();
            p.full = true;
            self.full = true;
        }
    }

    /// The current workspace's minimized rows, where the layout put them.
    pub(super) fn draw_minimized(&mut self) {
        let rows = self.mini_rows.clone();
        self.mini_hits.clear();
        if rows.is_empty() {
            return;
        }
        let mut ids = Vec::new();
        self.ws().root.panes(&mut ids);
        let c = self.cur_theme.chrome();
        let scale = self.scale;
        let px = |v: f32| (v * scale).round() as i32;
        let sh = self.fonts.small.height;
        let mh = self.fonts.mono.height;
        for (id, r) in rows {
            let Some(p) = self.panes.get(&id) else { continue };
            let h = r.h;
            let number = ids.iter().position(|p| *p == id).map_or(0, |n| n + 1);
            let lit = self.mini_lit == Some(id);
            self.canvas.fill(r, if lit { mix(c.side, c.text, 10) } else { c.side });
            self.canvas.fill(Rect { y: r.y + r.h - 1, h: 1, ..r }, c.edge);
            // How it is doing: running green, its last command failed red,
            // else quiet.
            let running = p.shell.running.is_some() && !p.shell.at_prompt;
            let failed = !running && p.shell.blocks.last().is_some_and(|b| b.exit.is_some_and(|e| e != 0));
            let dot = if running { 0x30A46C } else if failed { 0xE5484D } else { c.faint };
            let d = px(8.0);
            let mut x = r.x + px(10.0);
            self.canvas.round_fill(Rect { x, y: r.y + (h - d) / 2, w: d, h: d }, d / 2, dot, 255);
            x += d + px(10.0);
            x = self.canvas.text(&mut self.fonts.small, &format!("{number}"), x as f32, r.y + (h - sh) / 2, c.faint, r) + px(10.0);
            let cmd = p.shell.running.as_ref().filter(|_| running).map(|r| r.2.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| p.caption.clone());
            let (line, elapsed) = (p.last_line.clone(), p.shell.running.as_ref().filter(|_| running).map(|r| commands::human(r.0.elapsed())));
            // The command, its last line, how long, then restore.
            let cmd_w = self.fonts.mono.width(&cmd).min(r.w / 3);
            self.canvas.text_fit(&mut self.fonts.mono, &cmd, x, r.y + (h - mh) / 2, cmd_w, c.text, r);
            x += cmd_w + px(14.0);
            let ib = h - px(6.0);
            let right = r.x + r.w - px(8.0) - ib;
            let time = elapsed.unwrap_or_default();
            let tw = self.fonts.small.width(&time);
            let room = right - px(12.0) - tw - x;
            let bad = ["error", "failed", "panic"].iter().any(|w| line.to_lowercase().contains(w));
            self.canvas.text_fit(&mut self.fonts.mono, &line, x, r.y + (h - mh) / 2, room.max(0), if bad { 0xF2777A } else { c.dim }, r);
            self.canvas.text(&mut self.fonts.small, &time, (right - px(8.0) - tw) as f32, r.y + (h - sh) / 2, c.faint, r);
            let b = Rect { x: right, y: r.y + (h - ib) / 2, w: ib, h: ib };
            let size = ib * 6 / 10;
            self.canvas.icon(crate::canvas::icons::FOCUS, size, b.x + (ib - size) / 2, b.y + (ib - size) / 2, if lit { c.text } else { c.dim }, r);
            self.mini_hits.push((r, id));
            self.damage.push(r);
        }
    }

    /// A click on a minimized row: the pane back. True when it was one.
    pub(super) fn mini_click(&mut self) -> bool {
        let (x, y) = self.pointer;
        let Some(id) = self.mini_hits.iter().find(|(r, _)| r.contains(x, y)).map(|(_, id)| *id) else { return false };
        self.restore(id);
        true
    }

    /// The pointer over a minimized row: lit, and the hand.
    pub(super) fn mini_hover(&mut self, x: i32, y: i32) -> bool {
        let at = self.mini_hits.iter().find(|(r, _)| r.contains(x, y)).map(|(_, id)| *id);
        if at != self.mini_lit {
            self.mini_lit = at;
            self.full = true;
            self.redraw();
        }
        if at.is_some() {
            if let Some(w) = &self.window {
                w.set_cursor(CursorIcon::Pointer);
            }
        }
        at.is_some()
    }

    /// A right click on a minimized row: what can be done with the pane.
    pub(super) fn mini_menu(&mut self) -> bool {
        let (x, y) = self.pointer;
        let Some(id) = self.mini_hits.iter().find(|(r, _)| r.contains(x, y)).map(|(_, id)| *id) else { return false };
        let running = self.panes.get(&id).is_some_and(|p| p.shell.running.is_some() && !p.shell.at_prompt);
        let has_line = self.panes.get(&id).is_some_and(|p| !p.last_line.is_empty());
        use crate::palette::{Action, Item};
        let c = |label: &str, hint: &str, action| Some(Item { label: label.into(), hint: hint.into(), action });
        self.open_items_menu(vec![
            c("Restore", "click", Action::MiniRestore(id)),
            c("Focus pane", "", Action::MiniFocus(id)),
            None,
            if running { c("Restart", "", Action::MiniRestart(id)) } else { None },
            if running { c("Stop", "Ctrl C", Action::MiniStop(id)) } else { None },
            if has_line { c("Copy last line", "", Action::MiniCopyLine(id)) } else { None },
            None,
            c("Close pane", "", Action::MiniClose(id)),
        ]);
        true
    }
}
