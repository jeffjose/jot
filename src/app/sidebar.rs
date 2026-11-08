//! The sidebar, at the left of the window: the machines, and under each
//! what is on it. Its workspaces in this window, each opened or closed by
//! its chevron, their panes under them; its panes that sit in a workspace
//! under another machine; and the workspaces and panes of it no window
//! here shows. A click brings any of them into view; nothing is said about
//! which are shown, as all of them are running all the same.
//!
//! At its top, Machines and + to add one; at its foot, Settings. The
//! button at its top left tucks it away (it then takes no room: the button
//! moves into the top-left pane's caption); its edge drags it wider or
//! narrower, and dragged narrow it tucks away.

use super::*;
use crate::canvas::icons;

/// What can be clicked in the sidebar.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SideHit {
    Toggle,
    AddMachine,
    /// A new workspace on this machine.
    NewWorkspace(usize),
    Workspace(usize),
    /// A workspace's chevron: its panes shown under it, or not.
    Chevron(usize),
    Pane(PaneId),
    /// One of a machine's workspaces or panes this window does not show.
    Listed(usize, usize),
    Settings,
}

/// Its width when opened, in points, unless dragged otherwise.
pub const WIDTH: f32 = 260.0;
/// Narrowest it is drawn; dragged narrower than `CLOSE` it tucks away.
const NARROWEST: f32 = 180.0;
const CLOSE: f32 = 110.0;

/// This machine's name, as the sidebar heads its workspaces with.
fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname").map(|h| h.trim().to_string()).unwrap_or_else(|_| "this machine".into())
}

/// A name for a new workspace: two words, as "amber-harbor".
pub fn random_name() -> String {
    const FIRST: &[&str] = &[
        "amber", "quiet", "copper", "slow", "bright", "silver", "misty", "brave", "lucky", "hidden", "golden", "gentle", "rapid", "solar", "frosty", "velvet", "crimson", "mellow", "nimble", "sunny", "cobalt", "dusky", "rustic", "lunar", "wild", "calm", "swift", "plain", "early", "salty",
    ];
    const SECOND: &[&str] = &[
        "harbor", "fern", "moth", "river", "kiln", "otter", "meadow", "comet", "lantern", "pebble", "maple", "falcon", "canyon", "ember", "willow", "orchid", "tide", "summit", "badger", "cedar", "prairie", "beacon", "sparrow", "glacier", "thistle", "marsh", "quarry", "heron", "grove", "delta",
    ];
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64) ^ std::process::id() as u64;
    let mix = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    format!("{}-{}", FIRST[(mix >> 7) as usize % FIRST.len()], SECOND[(mix >> 29) as usize % SECOND.len()])
}

impl App {
    /// How much of the window's left it takes: none, tucked away.
    pub(super) fn sidebar_w(&self) -> i32 {
        if self.sidebar_open {
            self.px(self.sidebar_width)
        } else {
            0
        }
    }

    pub(super) fn toggle_sidebar(&mut self) {
        self.sidebar_open = !self.sidebar_open;
        if self.sidebar_open && self.sidebar_width < NARROWEST {
            self.sidebar_width = WIDTH;
        }
        persist::save_sidebar(self.sidebar_open, self.sidebar_width);
        self.side_lit = None;
        self.side_toggle = None;
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
    pub(super) fn ws_machine(&self, wi: usize) -> usize {
        let mut ids = Vec::new();
        self.workspaces[wi].root.panes(&mut ids);
        let first = ids.first().map_or(0, |id| crate::mux::split_id(*id).0);
        if ids.iter().all(|id| crate::mux::split_id(*id).0 == first) {
            first
        } else {
            0
        }
    }

    /// The button to bring it back, tucked away: in the caption of the
    /// pane at the top left (`bar`), or over its corner without captions.
    /// Returns where the caption's own content starts.
    pub(super) fn draw_side_button(&mut self, bar: Rect) -> i32 {
        let c = self.cur_theme.chrome();
        let b = (bar.h - self.px(4.0)).max(self.px(18.0));
        let r = Rect { x: bar.x + self.px(3.0), y: bar.y + (bar.h - b) / 2, w: b, h: b };
        let lit = self.side_lit == Some(SideHit::Toggle);
        let lift = if self.cur_theme.dark { 0xFFFFFF } else { 0x000000 };
        self.canvas.round_fill(r, self.px(5.0), lift, if lit { 30 } else { 14 });
        self.canvas.icon_in(icons::SIDEBAR, r, self.px(13.0), if lit { c.text } else { c.dim });
        self.side_toggle = Some(r);
        r.x + r.w + self.px(6.0)
    }

    pub(super) fn draw_sidebar(&mut self) {
        self.side_hits.clear();
        if !self.sidebar_open {
            // Without captions there is no caption to hold the button: it
            // floats over the top-left corner.
            if !self.config.captions {
                let a = self.area();
                let bar = Rect { x: a.x, y: a.y, w: self.px(30.0), h: self.px(28.0) };
                self.draw_side_button(bar);
                self.damage.push(bar);
            }
            return;
        }
        self.side_toggle = None;
        let c = self.cur_theme.chrome();
        let w = self.sidebar_w();
        let h = self.canvas.height - self.status_h();
        let area = Rect { x: 0, y: 0, w, h };
        self.canvas.fill(area, c.side);
        self.canvas.fill(Rect { x: w - 1, y: 0, w: 1, h }, c.edge);
        let scale = self.scale;
        let px = |v: f32| (v * scale).round() as i32;
        let (nh, dh) = (self.fonts.side.height, self.fonts.side_small.height);
        let row_h = nh + px(14.0);
        let lit = self.side_lit;
        // The quiet steps: a row under the pointer, the workspace you are
        // in, and the pane you are in.
        let (hover, here, focus_fill) = (mix(c.side, c.text, 5), mix(c.side, c.text, 7), mix(c.side, c.text, 11));
        let clip = Rect { x: 0, y: 0, w: w - 1, h };
        let pad = px(8.0);
        let inner_w = w - 2 * pad;
        let ih = px(14.0);
        let b = px(28.0);
        let button = |app: &mut App, r: Rect, hit: SideHit, icon: char, shown: bool| {
            let on = lit == Some(hit);
            if on {
                app.canvas.round_fill(r, px(6.0), hover, 255);
            }
            if shown || on {
                app.canvas.icon_in(icon, r, ih, if on { c.text } else { c.dim });
            }
            app.side_hits.push((r, hit));
        };
        // The top: the button, Machines, and + to add one.
        let top = px(6.0);
        button(self, Rect { x: pad - px(2.0), y: top, w: b, h: b }, SideHit::Toggle, icons::SIDEBAR, true);
        self.canvas.text(&mut self.fonts.side_small, "Machines", (pad - px(2.0) + b + px(8.0)) as f32, top + (b - dh) / 2, c.faint, clip);
        button(self, Rect { x: w - pad - b, y: top, w: b, h: b }, SideHit::AddMachine, icons::PLUS, true);
        // The foot: Settings, alone.
        let foot = Rect { x: pad, y: h - row_h - px(8.0), w: inner_w, h: row_h };
        self.canvas.fill(Rect { x: 0, y: foot.y - px(6.0), w: w - 1, h: 1 }, c.edge);
        if lit == Some(SideHit::Settings) {
            self.canvas.round_fill(foot, px(7.0), hover, 255);
        }
        let s_color = if lit == Some(SideHit::Settings) { c.text } else { c.dim };
        self.canvas.icon_in(icons::SETTINGS, Rect { x: foot.x + px(6.0), y: foot.y + (row_h - ih) / 2, w: ih, h: ih }, ih, s_color);
        self.canvas.text(&mut self.fonts.side, "Settings", (foot.x + px(6.0) + ih + px(10.0)) as f32, foot.y + (row_h - nh) / 2, s_color, clip);
        let kw = self.fonts.side_small.width("Ctrl ,");
        self.canvas.text(&mut self.fonts.side_small, "Ctrl ,", (foot.x + foot.w - px(8.0) - kw) as f32, foot.y + (row_h - dh) / 2, c.faint, clip);
        self.side_hits.push((foot, SideHit::Settings));
        let bottom = foot.y - px(10.0);
        let mut y = top + b + px(10.0);
        let accents = self.ws_accents();
        for k in 0..self.machines.len() {
            let shown = self.panes.keys().filter(|id| crate::mux::split_id(**id).0 == k).count();
            // A machine is listed once added, or while it holds anything of
            // this window's: a pane shown, or one closed and running on.
            if k > 0 && !self.machines[k].added && shown == 0 && self.machines[k].listing.is_empty() {
                continue;
            }
            if y > bottom - row_h {
                break;
            }
            if k > 0 {
                y += px(12.0);
            }
            // Its heading: its icon, its name, how it is, and + on hover.
            let head = Rect { x: pad, y, w: inner_w, h: row_h - px(4.0) };
            let (icon, name, note, dot) = if k == 0 && self.machines[0].host.is_none() {
                (icons::MONITOR, hostname(), "this machine".to_string(), None)
            } else {
                let up = self.machines[k].client.connected();
                (icons::SERVER, self.machines[k].host.clone().unwrap_or_default(), String::new(), Some(if up { 0x3DBB7E } else { 0xF0B43C }))
            };
            self.canvas.icon_in(icon, Rect { x: head.x + px(4.0), y: head.y + (head.h - ih) / 2, w: ih, h: ih }, ih, if k > 0 { 0xF0B43C } else { c.faint });
            let mut x = self.canvas.text(&mut self.fonts.side_small, &name, (head.x + px(4.0) + ih + px(9.0)) as f32, head.y + (head.h - dh) / 2, c.dim, clip) + px(8.0);
            if let Some(d) = dot {
                let s = px(7.0);
                self.canvas.round_fill(Rect { x, y: head.y + (head.h - s) / 2, w: s, h: s }, s / 2, d, 255);
                x += s + px(8.0);
            }
            if !note.is_empty() {
                self.canvas.text(&mut self.fonts.side_small, &note, x as f32, head.y + (head.h - dh) / 2, c.faint, clip);
            }
            let plus = Rect { x: head.x + head.w - px(26.0), y: head.y + (head.h - px(24.0)) / 2, w: px(24.0), h: px(24.0) };
            let near = self.side_heading == Some(k);
            button(self, plus, SideHit::NewWorkspace(k), icons::PLUS, near);
            y += head.h;
            // Its workspaces in this window, each opened by its chevron.
            for wi in 0..self.workspaces.len() {
                if self.ws_machine(wi) != k || y > bottom - row_h {
                    continue;
                }
                let mut ids = Vec::new();
                self.workspaces[wi].root.panes(&mut ids);
                let (name, count, open) = (self.workspaces[wi].name.clone(), ids.len(), self.workspaces[wi].open);
                let current = wi == self.current;
                let row = Rect { x: pad, y, w: inner_w, h: row_h };
                if current {
                    self.canvas.round_fill(row, px(7.0), here, 255);
                } else if lit == Some(SideHit::Workspace(wi)) {
                    self.canvas.round_fill(row, px(7.0), hover, 255);
                }
                let chev = Rect { x: row.x, y: row.y, w: px(22.0), h: row_h };
                let cv = px(11.0);
                let chev_icon = if open { icons::CHEVRON_DOWN } else { icons::CHEVRON_RIGHT };
                self.canvas.icon_in(chev_icon, Rect { x: chev.x + (chev.w - cv) / 2, y: chev.y + (row_h - cv) / 2, w: cv, h: cv }, cv, if lit == Some(SideHit::Chevron(wi)) { c.text } else { c.faint });
                let sq = px(9.0);
                let color = accents.get(wi).map_or(self.accent, |a| a.0);
                self.canvas.round_fill(Rect { x: chev.x + chev.w + px(2.0), y: y + (row_h - sq) / 2, w: sq, h: sq }, px(3.0), color, 255);
                let tx = chev.x + chev.w + px(2.0) + sq + px(9.0);
                let n = format!("{count}");
                let cw = self.fonts.side_small.width(&n);
                self.canvas.text_fit(&mut self.fonts.side, &name, tx, y + (row_h - nh) / 2, row.x + row.w - px(18.0) - cw - tx, if current { c.text } else { c.dim }, clip);
                self.canvas.text(&mut self.fonts.side_small, &n, (row.x + row.w - px(10.0) - cw) as f32, y + (row_h - dh) / 2, c.faint, clip);
                self.side_hits.push((chev, SideHit::Chevron(wi)));
                self.side_hits.push((row, SideHit::Workspace(wi)));
                y += row_h;
                if open {
                    y = self.draw_sidebar_panes(wi, k, y, row_h, clip, focus_fill, hover);
                }
            }
            // Its panes in a workspace under another machine.
            let mut elsewhere: Vec<PaneId> = Vec::new();
            for wi in 0..self.workspaces.len() {
                if self.ws_machine(wi) == k {
                    continue;
                }
                let mut ids = Vec::new();
                self.workspaces[wi].root.panes(&mut ids);
                elsewhere.extend(ids.into_iter().filter(|id| crate::mux::split_id(*id).0 == k));
            }
            for id in elsewhere {
                if y > bottom - row_h {
                    break;
                }
                let (label, running) = self.panes.get(&id).map_or((String::new(), false), |p| (p.caption.clone(), p.shell.running.is_some() && !p.shell.at_prompt));
                let focused = Some(id) == self.focused();
                y = self.sidebar_row(y, row_h, px(30.0), &label, running, SideHit::Pane(id), focused, clip, focus_fill, hover);
            }
            // Its workspaces and panes no window here shows: a click brings
            // one into view.
            let listing: Vec<(String, usize, bool, bool)> = self.machines[k]
                .listing
                .iter()
                .map(|l| {
                    if l.label.is_empty() {
                        let name = toml::from_str::<WorkspaceSpec>(&l.spec).map(|s| s.name).unwrap_or_default();
                        (if name.is_empty() { "workspace".into() } else { name }, l.panes.len(), false, false)
                    } else {
                        (l.label.clone(), 1, true, l.running)
                    }
                })
                .collect();
            for (i, (name, count, pane, running)) in listing.into_iter().enumerate() {
                if y > bottom - row_h {
                    break;
                }
                let hit = SideHit::Listed(k, i);
                if pane {
                    y = self.sidebar_row(y, row_h, px(30.0), &name, running, hit, false, clip, focus_fill, hover);
                    continue;
                }
                let row = Rect { x: pad, y, w: inner_w, h: row_h };
                if lit == Some(hit) {
                    self.canvas.round_fill(row, px(7.0), hover, 255);
                }
                let sq = px(9.0);
                self.canvas.round_fill(Rect { x: row.x + px(24.0), y: y + (row_h - sq) / 2, w: sq, h: sq }, px(3.0), mix(c.side, c.text, 30), 255);
                let tx = row.x + px(24.0) + sq + px(9.0);
                let n = format!("{count}");
                let cw = self.fonts.side_small.width(&n);
                self.canvas.text_fit(&mut self.fonts.side, &name, tx, y + (row_h - nh) / 2, row.x + row.w - px(18.0) - cw - tx, c.dim, clip);
                self.canvas.text(&mut self.fonts.side_small, &n, (row.x + row.w - px(10.0) - cw) as f32, y + (row_h - dh) / 2, c.faint, clip);
                self.side_hits.push((row, hit));
                y += row_h;
            }
        }
        self.damage.push(area);
    }

    /// A pane's row: its dot (running green), its label, lit as asked.
    #[allow(clippy::too_many_arguments)]
    fn sidebar_row(&mut self, y: i32, row_h: i32, indent: i32, label: &str, running: bool, hit: SideHit, focused: bool, clip: Rect, focus_fill: u32, hover: u32) -> i32 {
        let c = self.cur_theme.chrome();
        let scale = self.scale;
        let px = |v: f32| (v * scale).round() as i32;
        let w = clip.w + 1;
        let row = Rect { x: px(8.0), y, w: w - px(16.0), h: row_h - px(2.0) };
        if focused {
            self.canvas.round_fill(row, px(7.0), focus_fill, 255);
        } else if self.side_lit == Some(hit) {
            self.canvas.round_fill(row, px(7.0), hover, 255);
        }
        let d = px(7.0);
        let x = row.x + indent;
        self.canvas.round_fill(Rect { x, y: y + (row.h - d) / 2, w: d, h: d }, d / 2, if running { 0x3DBB7E } else { mix(c.side, c.text, 30) }, 255);
        let nh = self.fonts.side.height;
        let tx = x + d + px(9.0);
        self.canvas.text_fit(&mut self.fonts.side, label, tx, y + (row.h - nh) / 2, row.x + row.w - px(10.0) - tx, if focused { c.text } else { c.dim }, clip);
        self.side_hits.push((row, hit));
        y + row.h
    }

    /// A workspace's panes, under it: number, how it is doing (running
    /// green, failed red), its caption, and what it runs; a pane on another
    /// machine than `k` with that machine's name. Returns where they end.
    #[allow(clippy::too_many_arguments)]
    fn draw_sidebar_panes(&mut self, wi: usize, k: usize, mut y: i32, row_h: i32, clip: Rect, focus_fill: u32, hover: u32) -> i32 {
        let c = self.cur_theme.chrome();
        let scale = self.scale;
        let px = |v: f32| (v * scale).round() as i32;
        let (nh, dh, mh) = (self.fonts.side.height, self.fonts.side_small.height, self.fonts.mono.height);
        let mut ids = Vec::new();
        self.workspaces[wi].root.panes(&mut ids);
        let focus = if wi == self.current { self.focused() } else { None };
        let w = clip.w + 1;
        let prow = row_h - px(2.0);
        for (i, id) in ids.iter().enumerate() {
            let Some(p) = self.panes.get(id) else { continue };
            let running = p.shell.running.as_ref().filter(|_| !p.shell.at_prompt).map(|r| r.2.trim().to_string());
            let failed = running.is_none() && p.shell.blocks.last().is_some_and(|b| b.exit.is_some_and(|e| e != 0));
            let dot = if running.is_some() { 0x3DBB7E } else if failed { 0xE5484D } else { mix(c.side, c.text, 30) };
            let caption = p.caption.clone();
            let m = crate::mux::split_id(*id).0;
            let host = (m > 0 && m != k).then(|| self.machines.get(m).and_then(|x| x.host.clone())).flatten();
            let row = Rect { x: px(8.0), y, w: w - px(16.0), h: prow };
            let focused = focus == Some(*id);
            if focused {
                self.canvas.round_fill(row, px(7.0), focus_fill, 255);
            } else if self.side_lit == Some(SideHit::Pane(*id)) {
                self.canvas.round_fill(row, px(7.0), hover, 255);
            }
            let mut x = row.x + px(34.0);
            let num = format!("{}", i + 1);
            x = self.canvas.text(&mut self.fonts.side_small, &num, x as f32, y + (prow - dh) / 2, if focused { self.cur_accent } else { c.faint }, clip) + px(9.0);
            let d = px(7.0);
            self.canvas.round_fill(Rect { x, y: y + (prow - d) / 2, w: d, h: d }, d / 2, dot, 255);
            x += d + px(9.0);
            let right = row.x + row.w - px(8.0);
            let mut end = right;
            if let Some(host) = &host {
                let tw = self.fonts.side_small.width(host) + px(12.0);
                let pill = Rect { x: right - tw, y: y + (prow - (dh + px(6.0))) / 2, w: tw, h: dh + px(6.0) };
                self.canvas.round_fill(pill, px(4.0), 0xF0B43C, 30);
                self.canvas.text(&mut self.fonts.side_small, host, (pill.x + px(6.0)) as f32, pill.y + px(3.0), 0xF0B43C, clip);
                end = pill.x - px(6.0);
            }
            let room = end - x;
            let cw = self.fonts.side.width(&caption).min(room * 6 / 10);
            let after = self.canvas.text_fit(&mut self.fonts.side, &caption, x, y + (prow - nh) / 2, cw.max(0), if focused { c.text } else { c.dim }, clip);
            if let Some(cmd) = running {
                let x2 = after + px(8.0);
                self.canvas.text_fit(&mut self.fonts.mono, &cmd, x2, y + (prow - mh) / 2, (end - x2).max(0), c.faint, clip);
            }
            self.side_hits.push((row, SideHit::Pane(*id)));
            y += prow;
        }
        y + px(4.0)
    }

    /// A new workspace on machine `k`: a shell in its home there (here, in
    /// the folder of the pane you are in).
    fn new_workspace_on(&mut self, k: usize) {
        let host = (k > 0).then(|| self.machines.get(k).and_then(|m| m.host.clone())).flatten();
        let cwd = if k > 0 { "~".to_string() } else { self.focused().and_then(|f| self.panes.get(&f)).and_then(|p| p.pty.cwd()).unwrap_or_else(pty::home).display().to_string() };
        let before = self.workspaces.len();
        self.add_workspace(&WorkspaceSpec { host, cwd: Some(cwd), ..Default::default() });
        if self.workspaces.len() > before {
            self.go_to(before);
        }
        self.unsaved = true;
    }

    /// What is under the pointer: the narrower first, a chevron or a + over
    /// its row.
    fn side_hit_at(&self, x: i32, y: i32) -> Option<SideHit> {
        self.side_hits.iter().filter(|(r, _)| r.contains(x, y)).min_by_key(|(r, _)| r.w).map(|(_, h)| *h)
    }

    /// A click in the sidebar, or on its button tucked away. True when it
    /// was on it.
    pub(super) fn sidebar_click(&mut self) -> bool {
        let (x, y) = self.pointer;
        if self.side_toggle.is_some_and(|r| r.contains(x, y)) {
            self.toggle_sidebar();
            return true;
        }
        if x >= self.sidebar_w() {
            return false;
        }
        match self.side_hit_at(x, y) {
            Some(SideHit::Toggle) => self.toggle_sidebar(),
            Some(SideHit::AddMachine) => self.add_machine_prompt(),
            Some(SideHit::NewWorkspace(k)) => self.new_workspace_on(k),
            Some(SideHit::Workspace(wi)) => self.go_to(wi),
            Some(SideHit::Chevron(wi)) => {
                if let Some(ws) = self.workspaces.get_mut(wi) {
                    ws.open = !ws.open;
                }
                self.full = true;
                self.redraw();
            }
            Some(SideHit::Listed(k, i)) => self.open_listed(k, i),
            Some(SideHit::Settings) => self.open_settings(),
            Some(SideHit::Pane(id)) => {
                if let Some(wi) = self.workspaces.iter().position(|w| w.root.contains(id)) {
                    if wi != self.current {
                        self.go_to(wi);
                    }
                }
                self.set_focus(id);
                self.full = true;
                self.redraw();
            }
            None => {}
        }
        true
    }

    /// The pointer over the sidebar: what it is on lit, the hand on it, a
    /// heading's +; on its edge, the resize arrows. True when it is over it.
    pub(super) fn sidebar_hover(&mut self, x: i32, y: i32) -> bool {
        if self.sidebar_edge_at(x) {
            if let Some(w) = &self.window {
                w.set_cursor(CursorIcon::EwResize);
            }
            return true;
        }
        let on_toggle = self.side_toggle.is_some_and(|r| r.contains(x, y));
        let inside = x < self.sidebar_w() || on_toggle;
        let at = if on_toggle { Some(SideHit::Toggle) } else if inside { self.side_hit_at(x, y) } else { None };
        let heading = if inside { self.heading_under(y) } else { None };
        if at != self.side_lit || heading != self.side_heading {
            self.side_lit = at;
            self.side_heading = heading;
            self.full = true;
            self.redraw();
        }
        if !inside {
            return false;
        }
        if let Some(w) = &self.window {
            w.set_cursor(if at.is_some() { CursorIcon::Pointer } else { CursorIcon::Default });
        }
        true
    }

    /// Which machine's heading row is at `y`, by its + button's row.
    fn heading_under(&self, y: i32) -> Option<usize> {
        let slack = self.px(6.0);
        self.side_hits.iter().find_map(|(r, h)| match h {
            SideHit::NewWorkspace(k) if y >= r.y - slack && y < r.y + r.h + slack => Some(*k),
            _ => None,
        })
    }
}
