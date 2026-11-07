//! The sidebar, at the left of the window: the machines, the workspaces,
//! and the panes of the workspace you are in, each what it runs and how
//! it is doing. A click goes to a workspace or a pane. It tucks away to a
//! thin rail with the button at its top left, or is dragged closed by its
//! edge; dragged wider or narrower, it stays so.

use super::*;
use crate::canvas::icons;

/// What can be clicked in the sidebar.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SideHit {
    Toggle,
    Workspace(usize),
    Pane(PaneId),
}

/// Its width when opened, in points, unless dragged otherwise.
pub const WIDTH: f32 = 240.0;
/// Narrowest it is drawn; dragged narrower than `CLOSE` it tucks away.
const NARROWEST: f32 = 170.0;
const CLOSE: f32 = 110.0;

/// This machine's name, as the sidebar heads its workspaces with.
fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname").map(|h| h.trim().to_string()).unwrap_or_else(|_| "this machine".into())
}

impl App {
    /// The rail it tucks into: room for its button.
    fn rail_w(&self) -> i32 {
        self.px(30.0)
    }

    /// How much of the window's left it takes.
    pub(super) fn sidebar_w(&self) -> i32 {
        if self.sidebar_open {
            self.px(self.sidebar_width)
        } else {
            self.rail_w()
        }
    }

    pub(super) fn toggle_sidebar(&mut self) {
        self.sidebar_open = !self.sidebar_open;
        if self.sidebar_open && self.sidebar_width < NARROWEST {
            self.sidebar_width = WIDTH;
        }
        persist::save_sidebar(self.sidebar_open, self.sidebar_width);
        self.full = true;
        self.relayout();
    }

    /// Its right edge, where it is dragged from.
    pub(super) fn sidebar_edge_at(&self, x: i32) -> bool {
        self.sidebar_open && (x - self.sidebar_w()).abs() <= self.px(4.0)
    }

    /// Dragged to `x`: that wide, or tucked away when narrow.
    pub(super) fn sidebar_drag_to(&mut self, x: i32) {
        let w = x as f32 / self.scale;
        if w < CLOSE {
            self.sidebar_open = false;
            self.drag = None;
        } else {
            self.sidebar_width = w.clamp(NARROWEST, 520.0);
        }
        persist::save_sidebar(self.sidebar_open, self.sidebar_width);
        self.full = true;
        self.relayout();
    }

    pub(super) fn draw_sidebar(&mut self) {
        self.side_hits.clear();
        let c = self.cur_theme.chrome();
        let w = self.sidebar_w();
        let h = self.canvas.height - self.status_h();
        let area = Rect { x: 0, y: 0, w, h };
        self.canvas.fill(area, c.side);
        self.canvas.fill(Rect { x: w - 1, y: 0, w: 1, h }, c.edge);
        let scale = self.scale;
        let px = |v: f32| (v * scale).round() as i32;
        let (uh, sh, mh) = (self.fonts.ui.height, self.fonts.small.height, self.fonts.mono.height);
        let row_h = uh + px(10.0);
        let lit = self.side_lit;
        let lift = if self.cur_theme.dark { 0xFFFFFF } else { 0x000000 };
        // The button, top left: tucks it away, or brings it back.
        let b = px(24.0);
        let toggle = Rect { x: px(3.0), y: px(4.0), w: b, h: b };
        if lit == Some(SideHit::Toggle) {
            self.canvas.round_fill(toggle, px(5.0), lift, 24);
        }
        self.canvas.icon_in(icons::SIDEBAR, toggle, px(14.0), if lit == Some(SideHit::Toggle) { c.text } else { c.dim });
        self.side_hits.push((toggle, SideHit::Toggle));
        if !self.sidebar_open {
            self.damage.push(area);
            return;
        }
        let mut y = toggle.y + toggle.h + px(8.0);
        let left = px(10.0);
        let clip = Rect { x: 0, y: 0, w: w - 1, h };
        // A machine's heading: its icon, its name, and what it is.
        let heading = |app: &mut App, y: i32, icon: char, name: &str, note: &str, dot: Option<u32>| {
            let ih = px(12.0);
            app.canvas.icon_in(icon, Rect { x: left, y: y + (row_h - ih) / 2, w: ih, h: ih }, ih, c.faint);
            let x = app.canvas.text(&mut app.fonts.small, name, (left + ih + px(8.0)) as f32, y + (row_h - sh) / 2, c.dim, clip);
            let nw = app.fonts.small.width(note);
            let mut right = w - px(12.0);
            if let Some(d) = dot {
                let s = px(6.0);
                app.canvas.round_fill(Rect { x: right - s, y: y + (row_h - s) / 2, w: s, h: s }, s / 2, d, 255);
                right -= s + px(6.0);
            }
            if right - nw > x + px(8.0) {
                app.canvas.text(&mut app.fonts.small, note, (right - nw) as f32, y + (row_h - sh) / 2, c.faint, clip);
            }
        };
        heading(self, y, icons::MONITOR, &hostname(), "this machine", None);
        y += row_h;
        let accents = self.ws_accents();
        let names: Vec<(String, usize)> = self
            .workspaces
            .iter()
            .map(|ws| {
                let mut ids = Vec::new();
                ws.root.panes(&mut ids);
                (ws.name.clone(), ids.len())
            })
            .collect();
        for (wi, (name, count)) in names.into_iter().enumerate() {
            let current = wi == self.current;
            let row = Rect { x: px(6.0), y, w: w - px(12.0), h: row_h };
            if current {
                self.canvas.round_fill(row, px(6.0), lift, if self.cur_theme.dark { 14 } else { 10 });
            } else if lit == Some(SideHit::Workspace(wi)) {
                self.canvas.round_fill(row, px(6.0), lift, 8);
            }
            let sq = px(8.0);
            let color = accents.get(wi).map_or(self.accent, |a| a.0);
            self.canvas.round_fill(Rect { x: row.x + px(8.0), y: y + (row_h - sq) / 2, w: sq, h: sq }, px(2.5), color, 255);
            let tx = row.x + px(8.0) + sq + px(8.0);
            let n = format!("{count}");
            let nw = self.fonts.small.width(&n);
            self.canvas.text_fit(&mut self.fonts.ui, &name, tx, y + (row_h - uh) / 2, row.x + row.w - px(10.0) - nw - tx - px(8.0), if current { c.text } else { c.dim }, clip);
            self.canvas.text(&mut self.fonts.small, &n, (row.x + row.w - px(10.0) - nw) as f32, y + (row_h - sh) / 2, c.faint, clip);
            self.side_hits.push((row, SideHit::Workspace(wi)));
            y += row_h;
            if current {
                y = self.draw_sidebar_panes(y, row_h, clip);
            }
            if y > h - row_h {
                break;
            }
        }
        // The other machines this window has panes on.
        for k in 1..self.machines.len() {
            if y > h - 2 * row_h {
                break;
            }
            let shown = self.panes.keys().filter(|id| crate::mux::split_id(**id).0 == k).count();
            if shown == 0 {
                continue;
            }
            y += px(10.0);
            let host = self.machines[k].host.clone().unwrap_or_default();
            let up = self.machines[k].client.connected();
            let note = if !up { "reconnecting".to_string() } else if shown == 1 { "1 pane here".into() } else { format!("{shown} panes here") };
            heading(self, y, icons::SERVER, &host, &note, Some(if up { 0x3DBB7E } else { 0xF0B43C }));
            y += row_h;
        }
        let _ = mh;
        self.damage.push(area);
    }

    /// The panes of the workspace you are in, under it: number, how it is
    /// doing, its caption, and what it runs. Returns where they end.
    fn draw_sidebar_panes(&mut self, mut y: i32, row_h: i32, clip: Rect) -> i32 {
        let c = self.cur_theme.chrome();
        let scale = self.scale;
        let px = |v: f32| (v * scale).round() as i32;
        let (uh, sh, mh) = (self.fonts.ui.height, self.fonts.small.height, self.fonts.mono.height);
        let lift = if self.cur_theme.dark { 0xFFFFFF } else { 0x000000 };
        let mut ids = Vec::new();
        self.ws().root.panes(&mut ids);
        let focus = self.focused();
        let w = clip.w + 1;
        for (i, id) in ids.iter().enumerate() {
            let Some(p) = self.panes.get(id) else { continue };
            let running = p.shell.running.as_ref().filter(|_| !p.shell.at_prompt).map(|r| r.2.trim().to_string());
            let failed = running.is_none() && p.shell.blocks.last().is_some_and(|b| b.exit.is_some_and(|e| e != 0));
            let dot = if running.is_some() { 0x3DBB7E } else if failed { 0xE5484D } else { c.faint };
            let caption = p.caption.clone();
            let m = crate::mux::split_id(*id).0;
            let host = (m > 0).then(|| self.machines.get(m).and_then(|x| x.host.clone())).flatten();
            let row = Rect { x: px(6.0), y, w: w - px(12.0), h: row_h };
            let focused = focus == Some(*id);
            if focused {
                self.canvas.round_fill(row, px(6.0), lift, 10);
            } else if self.side_lit == Some(SideHit::Pane(*id)) {
                self.canvas.round_fill(row, px(6.0), lift, 8);
            }
            let mut x = row.x + px(24.0);
            let num = format!("{}", i + 1);
            x = self.canvas.text(&mut self.fonts.small, &num, x as f32, y + (row_h - sh) / 2, if focused { self.cur_accent } else { c.faint }, clip) + px(8.0);
            let d = px(6.0);
            self.canvas.round_fill(Rect { x, y: y + (row_h - d) / 2, w: d, h: d }, d / 2, dot, 255);
            x += d + px(8.0);
            let right = row.x + row.w - px(8.0);
            // On another machine: its name, amber, at the right.
            let mut end = right;
            if let Some(host) = &host {
                let tw = self.fonts.small.width(host) + px(10.0);
                let pill = Rect { x: right - tw, y: y + (row_h - (sh + px(4.0))) / 2, w: tw, h: sh + px(4.0) };
                self.canvas.round_fill(pill, px(4.0), 0xF0B43C, 30);
                self.canvas.text(&mut self.fonts.small, host, (pill.x + px(5.0)) as f32, pill.y + px(2.0), 0xF0B43C, clip);
                end = pill.x - px(6.0);
            }
            let room = end - x;
            let cw = self.fonts.ui.width(&caption).min(room * 6 / 10);
            let after = self.canvas.text_fit(&mut self.fonts.ui, &caption, x, y + (row_h - uh) / 2, cw.max(0), if focused { c.text } else { c.dim }, clip);
            if let Some(cmd) = running {
                let x2 = after + px(8.0);
                self.canvas.text_fit(&mut self.fonts.mono, &cmd, x2, y + (row_h - mh) / 2, (end - x2).max(0), c.faint, clip);
            }
            self.side_hits.push((row, SideHit::Pane(*id)));
            y += row_h;
        }
        y + px(4.0)
    }

    /// A click in the sidebar. True when it was in it.
    pub(super) fn sidebar_click(&mut self) -> bool {
        let (x, y) = self.pointer;
        if x >= self.sidebar_w() {
            return false;
        }
        match self.side_hits.iter().find(|(r, _)| r.contains(x, y)).map(|(_, h)| *h) {
            Some(SideHit::Toggle) => self.toggle_sidebar(),
            Some(SideHit::Workspace(wi)) => self.go_to(wi),
            Some(SideHit::Pane(id)) => {
                self.set_focus(id);
                self.full = true;
                self.redraw();
            }
            None => {}
        }
        true
    }

    /// The pointer over the sidebar: what it is on lit, the hand on it;
    /// on its edge, the resize arrows. True when it is over it.
    pub(super) fn sidebar_hover(&mut self, x: i32, y: i32) -> bool {
        if self.sidebar_edge_at(x) {
            if let Some(w) = &self.window {
                w.set_cursor(CursorIcon::EwResize);
            }
            return true;
        }
        if x >= self.sidebar_w() {
            if self.side_lit.take().is_some() {
                self.full = true;
                self.redraw();
            }
            return false;
        }
        let at = self.side_hits.iter().find(|(r, _)| r.contains(x, y)).map(|(_, h)| *h);
        if at != self.side_lit {
            self.side_lit = at;
            self.full = true;
            self.redraw();
        }
        if let Some(w) = &self.window {
            w.set_cursor(if at.is_some() { CursorIcon::Pointer } else { CursorIcon::Default });
        }
        true
    }
}
