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
    /// A machine's workspace this window does not show.
    Listed(usize, usize),
    AddMachine,
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

    /// The machine a workspace is on, for the sidebar: the one all its
    /// panes are on, else this one.
    fn ws_machine(&self, wi: usize) -> usize {
        let mut ids = Vec::new();
        self.workspaces[wi].root.panes(&mut ids);
        let first = ids.first().map_or(0, |id| crate::mux::split_id(*id).0);
        if ids.iter().all(|id| crate::mux::split_id(*id).0 == first) {
            first
        } else {
            0
        }
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
        let (uh, sh) = (self.fonts.ui.height, self.fonts.small.height);
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
        let left = px(10.0);
        let clip = Rect { x: 0, y: 0, w: w - 1, h };
        // Add machine, at the foot.
        let foot = Rect { x: px(6.0), y: h - row_h - px(8.0), w: w - px(12.0), h: row_h };
        self.canvas.fill(Rect { x: 0, y: foot.y - px(6.0), w: w - 1, h: 1 }, c.edge);
        if lit == Some(SideHit::AddMachine) {
            self.canvas.round_fill(foot, px(6.0), lift, 10);
        }
        let ih = px(12.0);
        let add_color = if lit == Some(SideHit::AddMachine) { c.text } else { c.dim };
        self.canvas.icon_in(icons::PLUS, Rect { x: foot.x + px(6.0), y: foot.y + (row_h - ih) / 2, w: ih, h: ih }, ih, add_color);
        self.canvas.text(&mut self.fonts.ui, "Add machine", (foot.x + px(6.0) + ih + px(8.0)) as f32, foot.y + (row_h - uh) / 2, add_color, clip);
        self.side_hits.push((foot, SideHit::AddMachine));
        let bottom = foot.y - px(10.0);
        let mut y = toggle.y + toggle.h + px(8.0);
        let accents = self.ws_accents();
        for k in 0..self.machines.len() {
            let shown = self.panes.keys().filter(|id| crate::mux::split_id(**id).0 == k).count();
            if k > 0 && !self.machines[k].added && shown == 0 {
                continue;
            }
            if y > bottom - row_h {
                break;
            }
            if k > 0 {
                y += px(10.0);
            }
            // Its heading: its icon, its name, and how it is.
            let (icon, name, note, dot) = if k == 0 && self.machines[0].host.is_none() {
                (icons::MONITOR, hostname(), "this machine".to_string(), None)
            } else {
                let up = self.machines[k].client.connected();
                let host = self.machines[k].host.clone().unwrap_or_default();
                (icons::SERVER, host, if up { String::new() } else { "connecting".into() }, Some(if up { 0x3DBB7E } else { 0xF0B43C }))
            };
            self.canvas.icon_in(icon, Rect { x: left, y: y + (row_h - ih) / 2, w: ih, h: ih }, ih, c.faint);
            let x = self.canvas.text(&mut self.fonts.small, &name, (left + ih + px(8.0)) as f32, y + (row_h - sh) / 2, c.dim, clip);
            let mut right = w - px(12.0);
            if let Some(d) = dot {
                let s = px(6.0);
                self.canvas.round_fill(Rect { x: right - s, y: y + (row_h - s) / 2, w: s, h: s }, s / 2, d, 255);
                right -= s + px(6.0);
            }
            let nw = self.fonts.small.width(&note);
            if !note.is_empty() && right - nw > x + px(8.0) {
                self.canvas.text(&mut self.fonts.small, &note, (right - nw) as f32, y + (row_h - sh) / 2, c.faint, clip);
            }
            y += row_h;
            // Its workspaces in this window.
            for wi in 0..self.workspaces.len() {
                if self.ws_machine(wi) != k || y > bottom - row_h {
                    continue;
                }
                let mut ids = Vec::new();
                self.workspaces[wi].root.panes(&mut ids);
                let (name, count) = (self.workspaces[wi].name.clone(), ids.len());
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
                let cw = self.fonts.small.width(&n);
                self.canvas.text_fit(&mut self.fonts.ui, &name, tx, y + (row_h - uh) / 2, row.x + row.w - px(18.0) - cw - tx, if current { c.text } else { c.dim }, clip);
                self.canvas.text(&mut self.fonts.small, &n, (row.x + row.w - px(10.0) - cw) as f32, y + (row_h - sh) / 2, c.faint, clip);
                self.side_hits.push((row, SideHit::Workspace(wi)));
                y += row_h;
                if current {
                    y = self.draw_sidebar_panes(y, row_h, clip);
                }
            }
            // Its workspaces this window does not show: a click opens one
            // here, as it is there.
            let listing: Vec<(String, usize, bool)> = self.machines[k]
                .listing
                .iter()
                .map(|l| {
                    let name = toml::from_str::<WorkspaceSpec>(&l.spec).map(|s| s.name).unwrap_or_default();
                    (if name.is_empty() { "workspace".into() } else { name }, l.panes.len(), l.elsewhere)
                })
                .collect();
            for (i, (name, count, elsewhere)) in listing.into_iter().enumerate() {
                if y > bottom - row_h {
                    break;
                }
                let row = Rect { x: px(6.0), y, w: w - px(12.0), h: row_h };
                if lit == Some(SideHit::Listed(k, i)) {
                    self.canvas.round_fill(row, px(6.0), lift, 8);
                }
                let sq = px(8.0);
                let sq_r = Rect { x: row.x + px(8.0), y: y + (row_h - sq) / 2, w: sq, h: sq };
                self.canvas.round_fill(sq_r, px(2.5), c.faint, 255);
                self.canvas.round_fill(Rect { x: sq_r.x + 1, y: sq_r.y + 1, w: sq - 2, h: sq - 2 }, px(2.0), c.side, 255);
                let tx = sq_r.x + sq + px(8.0);
                let hovered = lit == Some(SideHit::Listed(k, i));
                let note = match (elsewhere, hovered) {
                    (true, _) => "in another window".to_string(),
                    (false, true) => "Open".to_string(),
                    (false, false) if count == 1 => "1 pane".to_string(),
                    (false, false) => format!("{count} panes"),
                };
                let nw = self.fonts.small.width(&note);
                self.canvas.text_fit(&mut self.fonts.ui, &name, tx, y + (row_h - uh) / 2, row.x + row.w - px(18.0) - nw - tx, c.dim, clip);
                self.canvas.text(&mut self.fonts.small, &note, (row.x + row.w - px(10.0) - nw) as f32, y + (row_h - sh) / 2, if lit == Some(SideHit::Listed(k, i)) && !elsewhere { c.text } else { c.faint }, clip);
                self.side_hits.push((row, SideHit::Listed(k, i)));
                y += row_h;
            }
        }
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
        let under = self.ws_machine(self.current);
        let focus = self.focused();
        let w = clip.w + 1;
        for (i, id) in ids.iter().enumerate() {
            let Some(p) = self.panes.get(id) else { continue };
            let running = p.shell.running.as_ref().filter(|_| !p.shell.at_prompt).map(|r| r.2.trim().to_string());
            let failed = running.is_none() && p.shell.blocks.last().is_some_and(|b| b.exit.is_some_and(|e| e != 0));
            let dot = if running.is_some() { 0x3DBB7E } else if failed { 0xE5484D } else { c.faint };
            let caption = p.caption.clone();
            let m = crate::mux::split_id(*id).0;
            // Its machine, when not the one the workspace is under.
            let host = (m > 0 && m != under).then(|| self.machines.get(m).and_then(|x| x.host.clone())).flatten();
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
            Some(SideHit::Listed(k, i)) => self.open_listed(k, i),
            Some(SideHit::AddMachine) => self.add_machine_prompt(),
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
