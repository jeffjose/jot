//! The sidebar, at the left of the window: the machines, and under each
//! what is on it. Its workspaces in this window, each opened or closed by
//! a click, their panes under them on a hairline; its panes that sit in a workspace
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
    /// A machine's heading (its menu on a right click).
    Machine(usize),
    /// A new terminal on this machine.
    NewTerminal(usize),
    /// A terminal a pane here shows.
    Pane(PaneId),
    /// A terminal no pane here shows, by machine and its name there.
    Free(usize, u64),
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
        // The quake window's is its own, not saved for the others.
        if !crate::quake_mode() {
            persist::save_sidebar(self.sidebar_open, self.sidebar_width);
        }
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
        // The quake window's is its own, not saved for the others.
        if !crate::quake_mode() {
            persist::save_sidebar(self.sidebar_open, self.sidebar_width);
        }
        self.full = true;
        self.relayout();
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
            // No caption held the button (captions off, or empty space or a
            // minimized row at the top left): it floats over that corner.
            if self.side_toggle.is_none() {
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
        let h = self.canvas.height - self.bottom_h() - if self.quake_bar_on_top() { 0 } else { self.quake_bar_h() };
        // Under jot's title bar, when it has one.
        let y0 = self.title_h();
        let area = Rect { x: 0, y: y0, w, h: h - y0 };
        self.canvas.fill(area, c.side);
        self.canvas.fill(Rect { x: w - 1, y: y0, w: 1, h: h - y0 }, c.edge);
        let scale = self.scale;
        let px = |v: f32| (v * scale).round() as i32;
        let (nh, dh) = (self.fonts.side.height, self.fonts.side_small.height);
        let row_h = nh + px(14.0);
        let lit = self.side_lit;
        // The quiet steps: a row under the pointer, the workspace you are
        // in, and the pane you are in.
        let (hover, focus_fill) = (mix(c.side, c.text, 5), mix(c.side, c.text, 11));
        let clip = Rect { x: 0, y: y0, w: w - 1, h: h - y0 };
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
        let top = y0 + px(6.0);
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
        // Machine by machine, every terminal on it, in one flat list in the
        // order they were made: those a pane here shows (with its number
        // when it is in the workspace you are in) and those none does.
        let terms = self.side_terminals();
        let focus = self.focused();
        let srow = nh + px(10.0);
        for k in 0..self.machines.len() {
            let mine: Vec<&SideTerm> = terms.iter().filter(|t| t.k == k).collect();
            // A machine is listed once added, or while it holds a terminal.
            if k > 0 && !self.machines[k].added && mine.is_empty() {
                continue;
            }
            if y > bottom - row_h {
                break;
            }
            if k > 0 {
                y += px(10.0);
            }
            // Its heading: its icon, its name, how it is, and + for a new
            // terminal on it.
            let head = Rect { x: pad, y, w: inner_w, h: row_h - px(4.0) };
            let (icon, name, note, dot) = if k == 0 && self.machines[0].host.is_none() {
                (icons::MONITOR, hostname(), "this machine".to_string(), None)
            } else {
                let up = self.machines[k].client.connected();
                (icons::SERVER, self.machines[k].host.clone().unwrap_or_default(), String::new(), Some(if up { 0x3DBB7E } else { 0xF0B43C }))
            };
            // Every machine in the same quiet gray; a remote one's dot says
            // whether it is reached.
            self.canvas.icon_in(icon, Rect { x: head.x + px(4.0), y: head.y + (head.h - ih) / 2, w: ih, h: ih }, ih, c.faint);
            self.side_hits.push((head, SideHit::Machine(k)));
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
            button(self, plus, SideHit::NewTerminal(k), icons::PLUS, near);
            y += head.h;
            for t in mine {
                if y > bottom - srow {
                    break;
                }
                let focused = t.pane.zip(focus).is_some_and(|(p, f)| crate::mux::base_id(p) == crate::mux::base_id(f));
                y = self.side_term_row(t, y, srow, pad, inner_w, focused, clip, focus_fill, hover);
            }
        }
        self.damage.push(area);
    }

    /// What is under the pointer: the narrower first, a + over
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
            Some(SideHit::NewTerminal(k)) => self.new_terminal_on(k),
            // A terminal no pane shows: into the pane you are in, its own
            // running on; with Ctrl, in a new split beside it.
            // A terminal's row: held, to be dragged onto a pane, or let go
            // where it was, a click.
            Some(hit @ (SideHit::Free(..) | SideHit::Pane(_))) => self.drag = Some(Drag::Side(SideCarry { hit, from: (x, y), moving: false, over: None })),
            Some(SideHit::Settings) => self.open_settings(),
            // A machine's heading: its menu is on a right click.
            Some(SideHit::Machine(_)) | None => {}
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
            SideHit::NewTerminal(k) if y >= r.y - slack && y < r.y + r.h + slack => Some(*k),
            _ => None,
        })
    }
}

impl App {
    /// What the sidebar shows that changes on its own (whether each
    /// pane's last command failed, its caption, whether it runs one), as one
    /// number: drawn again when it changes, and only then.
    pub(super) fn sidebar_moved(&mut self) -> bool {
        if !self.sidebar_open {
            return false;
        }
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let mut ids: Vec<&PaneId> = self.panes.keys().collect();
        ids.sort();
        for id in ids {
            let p = &self.panes[id];
            let failed = p.shell.blocks.last().is_some_and(|b| b.exit.is_some_and(|e| e != 0));
            let running = p.shell.running.is_some() && !p.shell.at_prompt;
            (id, running, failed, p.caption.as_str()).hash(&mut h);
        }
        // The pane you are in, and the workspace: its row picked out.
        (self.focused(), self.current).hash(&mut h);
        let sig = h.finish();
        let moved = sig != self.side_sig;
        self.side_sig = sig;
        moved
    }
}

/// A terminal as the sidebar lists it: its machine and name in that
/// machine's session; the pane here that shows it, if one does; what to
/// call it; what it runs now; how it is (0 idle, 1 running, 2 failed); and
/// what to say at its right: its pane's number, or that another window
/// shows it.
pub(super) struct SideTerm {
    pub k: usize,
    pub sid: u64,
    pub pane: Option<PaneId>,
    pub label: String,
    pub cmd: Option<String>,
    pub status: u8,
    /// Shown by a pane in the workspace you are in.
    pub here: bool,
}

impl App {
    /// Every terminal, for the sidebar: those panes here show, and those
    /// none does, in the order their sessions made them.
    pub(super) fn side_terminals(&self) -> Vec<SideTerm> {
        let mut out = Vec::new();
        for (wi, ws) in self.workspaces.iter().enumerate() {
            let mut ids = Vec::new();
            ws.root.panes(&mut ids);
            ids.retain(|id| !ws.spaces.contains(id));
            for id in &ids {
                let Some(p) = self.panes.get(id) else { continue };
                let (k, sid) = crate::mux::split_id(*id);
                let here = wi == self.current;
                // A terminal in two panes is one row, the pane here first.
                if let Some(t) = out.iter_mut().find(|t: &&mut SideTerm| (t.k, t.sid) == (k, sid)) {
                    if here && !t.here {
                        t.here = true;
                        t.pane = Some(*id);
                    }
                    continue;
                }
                let running = terminals::pane_running(p);
                let failed = running.is_none() && p.shell.blocks.last().is_some_and(|b| b.exit.is_some_and(|e| e != 0));
                let status = if running.is_some() { 1 } else if failed { 2 } else { 0 };
                let label = self.terminal_label(*id);
                let cmd = running.filter(|r| r.split_whitespace().next() != Some(label.as_str()) || r.contains(' '));
                out.push(SideTerm { k, sid, pane: Some(*id), label, cmd, status, here });
            }
        }
        for f in self.free_terminals() {
            // As a pane here last showed it, while it still runs something,
            // or still does not, as then; the session's word only for one never
            // shown here, or one that has started or ended a program since.
            let seen = self.term_seen.get(&(f.k, f.sid)).filter(|s| f.running == (s.2 == 1));
            let (label, cmd, status) = match seen {
                Some((l, c, s)) => (l.clone(), c.clone(), *s),
                None => (f.label, None, u8::from(f.running)),
            };
            let cmd = cmd.or_else(|| f.elsewhere.then(|| "in another window".to_string()));
            out.push(SideTerm { k: f.k, sid: f.sid, pane: None, label: self.given_name(f.k, f.sid).unwrap_or(label), cmd, status, here: false });
        }
        // One on its way into a pane, in no list for the moment: its row
        // stays where it is, as it was.
        for (k, spec, _) in &self.taking {
            for sid in spec.layout.as_ref().map(persist::ids_in_order).unwrap_or_default() {
                if out.iter().any(|t| (t.k, t.sid) == (*k, sid)) {
                    continue;
                }
                let Some((l, c, s)) = self.term_seen.get(&(*k, sid)) else { continue };
                out.push(SideTerm { k: *k, sid, pane: None, label: self.given_name(*k, sid).unwrap_or_else(|| l.clone()), cmd: c.clone(), status: *s, here: false });
            }
        }
        // By machine, then by number (one new since numbers were last given
        // at the end).
        out.sort_by_key(|t| (t.k, self.term_numbers.get(&(t.k, t.sid)).copied().unwrap_or(usize::MAX), t.sid));
        out
    }

    /// A terminal's row: one line, its dot, its name, what it runs dimmed,
    /// and at the right its number; a terminal no pane shows a shade
    /// quieter.
    #[allow(clippy::too_many_arguments)]
    fn side_term_row(&mut self, t: &SideTerm, y: i32, h: i32, x0: i32, w: i32, focused: bool, clip: Rect, focus_fill: u32, hover: u32) -> i32 {
        let c = self.cur_theme.chrome();
        let scale = self.scale;
        let px = |v: f32| (v * scale).round() as i32;
        let row = Rect { x: x0, y, w, h };
        let hit = match t.pane {
            Some(id) => SideHit::Pane(id),
            None => SideHit::Free(t.k, t.sid),
        };
        if focused {
            self.canvas.round_fill(row, px(6.0), focus_fill, 255);
        } else if self.side_lit == Some(hit) {
            self.canvas.round_fill(row, px(6.0), hover, 255);
        }
        let d = px(6.0);
        let dot = match t.status {
            1 => 0x3DBB7E,
            2 => 0xE5484D,
            _ => mix(c.side, c.text, 60),
        };
        let mut x = row.x + px(10.0);
        self.canvas.round_fill(Rect { x, y: y + (h - d) / 2, w: d, h: d }, d / 2, dot, 255);
        x += d + px(10.0);
        // At the right: its number, which stays with it whatever pane shows
        // it; ringed when a pane here shows it.
        let mut right = row.x + row.w - px(8.0);
        if let Some(n) = self.term_numbers.get(&(t.k, t.sid)) {
            let badge = n.to_string();
            let sh = self.fonts.side_small.height;
            // All the same width, the number in the middle, so that they line up.
            let tw = self.fonts.side_small.width(&badge);
            let bw = (tw + px(10.0)).max(px(22.0));
            let chip = Rect { x: right - bw, y: y + (h - sh - px(4.0)) / 2, w: bw, h: sh + px(4.0) };
            if t.here {
                self.canvas.ring(chip, px(4.0), 1, mix(c.side, c.text, 40));
            }
            let color = if focused { c.text } else if t.here { c.dim } else { mix(c.side, c.dim, 160) };
            self.canvas.text(&mut self.fonts.side_small, &badge, (chip.x + (bw - tw) / 2) as f32, chip.y + px(2.0), color, clip);
            right = chip.x - px(6.0);
        }
        let nh = self.fonts.side.height;
        let shown = t.pane.is_some();
        let name_color = if focused { c.text } else if shown { c.dim } else { mix(c.side, c.dim, 200) };
        let room = right - x;
        let nw = self.fonts.side.width(&t.label).min(if t.cmd.is_some() { room * 6 / 10 } else { room });
        let after = self.canvas.text_fit(&mut self.fonts.side, &t.label, x, y + (h - nh) / 2, nw.max(0), name_color, clip);
        if let Some(cmd) = &t.cmd {
            let mh = self.fonts.mono.height;
            let x2 = after + px(8.0);
            self.canvas.text_fit(&mut self.fonts.mono, cmd, x2, y + (h - mh) / 2, (right - x2).max(0), c.faint, clip);
        }
        self.side_hits.push((row, hit));
        y + h
    }
}

impl App {
    /// A right click on a terminal's row: what can be done with it.
    pub(super) fn sidebar_menu(&mut self) -> bool {
        use crate::palette::{Action, Item};
        let (x, y) = self.pointer;
        if x >= self.sidebar_w() {
            return false;
        }
        let c = |label: &str, hint: &str, action| Some(Item { label: label.into(), hint: hint.into(), action });
        match self.side_hit_at(x, y) {
            // A terminal no pane shows.
            Some(SideHit::Free(k, sid)) => {
                self.open_items_menu(vec![
                    c("Show in this pane", "click", Action::ShowTerminal(k, sid)),
                    c("Show in a split beside", "Ctrl click", Action::SplitTerminal(k, sid)),
                    None,
                    c("Rename…", "", Action::RenameTerminal(k, sid)),
                    None,
                    c("Kill terminal", "", Action::KillFree(k, sid)),
                ]);
                true
            }
            // A terminal a pane here shows: gone to, and what can be done
            // with its pane.
            Some(SideHit::Pane(id)) => {
                if let Some(wi) = self.workspaces.iter().position(|w| w.root.contains(id)) {
                    if wi != self.current {
                        self.go_to(wi);
                    }
                }
                self.set_focus(id);
                let (k, sid) = crate::mux::split_id(id);
                self.open_items_menu(vec![
                    c("Rename…", "/rename", Action::RenameTerminal(k, sid)),
                    c(self.state_label(&Action::Zoom).unwrap_or_default(), "Alt Shift F", Action::Zoom),
                    c("Minimize", "", Action::Minimize),
                    c("Move to new window", "Alt Shift O", Action::PopOut),
                    None,
                    c("Close pane (it runs on)", "Alt X", Action::Close),
                    c("Kill terminal", "", Action::EndPane),
                ]);
                true
            }
            // A machine: a new terminal on it; reached again, or taken out
            // of the sidebar; the terminals on it no pane shows, ended.
            Some(SideHit::Machine(k)) => {
                let up = self.machines.get(k).is_some_and(|m| m.client.connected());
                let added = self.machines.get(k).is_some_and(|m| m.added);
                let free = self.free_terminals().into_iter().filter(|f| f.k == k && !f.elsewhere).count();
                let mut items = vec![c("New terminal", "+", Action::NewTerminalOn(k))];
                if k > 0 && !up {
                    items.push(c("Reconnect", "", Action::Reconnect(k)));
                }
                if free > 0 {
                    items.push(None);
                    items.push(c(&format!("Kill {free} terminal{} no pane shows", if free == 1 { "" } else { "s" }), "", Action::KillFreeOn(k)));
                }
                if k > 0 && added {
                    items.push(None);
                    items.push(c("Remove from sidebar", "", Action::RemoveMachine(k)));
                }
                self.open_items_menu(items);
                true
            }
            _ => false,
        }
    }
}

/// A terminal's row held: where the press was, whether it has moved far
/// enough to be a drag, and the pane under the pointer and where on it.
#[derive(Clone, Copy, Debug)]
pub(super) struct SideCarry {
    pub hit: SideHit,
    pub from: (i32, i32),
    pub moving: bool,
    pub over: Option<(PaneId, dnd::Zone)>,
}

impl App {
    /// A row clicked: a terminal no pane shows into the pane you are in (in
    /// a split beside it, with Ctrl), its own running on; a shown one gone
    /// to.
    fn side_row_click(&mut self, hit: SideHit) {
        match hit {
            SideHit::Free(k, sid) => {
                let place = match self.focused() {
                    Some(at) if self.mods.control_key() => terminals::Place::Split(at, Dir::Across, false),
                    Some(at) => terminals::Place::Into(at),
                    None => terminals::Place::Beside,
                };
                self.take_terminal(k, sid, place);
            }
            // On screen here (it has a number): its pane is gone to, nothing
            // moves (Ctrl: shown again, in a split beside the pane you are
            // in, as a file is open in two editors). Shown in another
            // workspace of this window: shown in the pane you are in too
            // (Ctrl: beside it), as a terminal no pane shows would; the
            // pane there keeps it, and the layout stays.
            SideHit::Pane(id) => {
                let (k, sid) = crate::mux::split_id(id);
                let at = self.focused().filter(|at| self.ws().root.contains(*at));
                let here = self.views(id).into_iter().find(|v| self.ws().root.contains(*v));
                match (at, here) {
                    (Some(at), _) if self.mods.control_key() => self.take_terminal(k, sid, terminals::Place::Split(at, Dir::Across, false)),
                    (Some(at), None) => self.take_terminal(k, sid, terminals::Place::Into(at)),
                    (at, Some(v)) => {
                        // A pane showing it in focus already stays so.
                        if !at.is_some_and(|a| crate::mux::base_id(a) == crate::mux::base_id(v)) {
                            self.set_focus(v);
                        }
                    }
                    (None, None) => {
                        if let Some(wi) = self.workspaces.iter().position(|w| w.root.contains(id)) {
                            self.go_to(wi);
                        }
                        self.set_focus(id);
                    }
                }
                self.full = true;
                self.redraw();
            }
            _ => {}
        }
    }

    /// A held row dragged: the pane under the pointer, and where on it.
    pub(super) fn side_drag_move(&mut self, x: i32, y: i32) {
        let Some(Drag::Side(mut c)) = self.drag else { return };
        if !c.moving && ((x - c.from.0).abs() + (y - c.from.1).abs()) < self.px(6.0) {
            return;
        }
        c.moving = true;
        let own = if let SideHit::Pane(id) = c.hit { Some(id) } else { None };
        c.over = self.pane_at(x, y).map(|(id, _)| id).or_else(|| self.space_at(x, y)).filter(|id| Some(*id) != own).and_then(|id| self.panes.get(&id).map(|p| (id, dnd::zone(p.rect, x, y))));
        self.drag = Some(Drag::Side(c));
        if let Some(w) = &self.window {
            w.set_cursor(CursorIcon::Grabbing);
        }
        self.full = true;
        self.redraw();
    }

    /// The row let go: on a pane, its terminal shown there (the middle) or
    /// beside it (an edge); where it was, a click.
    pub(super) fn side_drag_drop(&mut self, c: SideCarry) {
        if let Some(w) = &self.window {
            w.set_cursor(CursorIcon::Default);
        }
        let Some((target, z)) = c.over.filter(|_| c.moving) else {
            if !c.moving {
                self.side_row_click(c.hit);
            }
            self.full = true;
            return self.redraw();
        };
        let (dir, before) = match z {
            dnd::Zone::Left => (Dir::Across, true),
            dnd::Zone::Right => (Dir::Across, false),
            dnd::Zone::Top => (Dir::Down, true),
            _ => (Dir::Down, false),
        };
        match c.hit {
            SideHit::Free(k, sid) => {
                let place = if z == dnd::Zone::Swap { terminals::Place::Into(target) } else { terminals::Place::Split(target, dir, before) };
                self.take_terminal(k, sid, place);
            }
            // A shown one, in the workspace you are in: moved, as a pane is
            // by its caption.
            SideHit::Pane(id) if self.ws().root.contains(id) && self.ws().root.contains(target) => {
                self.carry_drop(dnd::Carry { pane: id, from: c.from, moving: true, over: Some((target, z)) });
            }
            // Shown in another workspace here: shown here too, there (the
            // middle of a pane) or beside it (an edge), the pane there
            // keeping it.
            SideHit::Pane(id) if self.ws().root.contains(target) => {
                let (k, sid) = crate::mux::split_id(id);
                let place = if z == dnd::Zone::Swap { terminals::Place::Into(target) } else { terminals::Place::Split(target, dir, before) };
                self.take_terminal(k, sid, place);
            }
            hit => self.side_row_click(hit),
        }
        self.full = true;
        self.redraw();
    }

    /// Where a dragged row would land, shaded in the accent, and its name
    /// beside the pointer.
    pub(super) fn draw_side_carry(&mut self) {
        let Some(Drag::Side(c)) = self.drag else { return };
        if !c.moving {
            return;
        }
        if let Some((target, z)) = c.over {
            if let Some(r) = self.panes.get(&target).map(|p| p.rect) {
                let r = dnd::zone_rect(r, z);
                let m = self.px(4.0);
                let inner = Rect { x: r.x + m, y: r.y + m, w: r.w - 2 * m, h: r.h - 2 * m };
                self.canvas.round_fill(inner, self.px(8.0), self.cur_accent, 70);
                self.canvas.ring(inner, self.px(8.0), self.px(2.0), self.cur_accent);
            }
        }
        let label = self.side_terminals().into_iter().find(|t| match c.hit {
            SideHit::Free(k, sid) => t.pane.is_none() && t.k == k && t.sid == sid,
            SideHit::Pane(id) => t.pane == Some(id),
            _ => false,
        });
        if let Some(t) = label {
            let ch = self.cur_theme.chrome();
            let h = self.fonts.side.height + self.px(12.0);
            let w = self.fonts.side.width(&t.label) + self.px(24.0);
            let r = Rect { x: (self.pointer.0 + self.px(14.0)).min(self.canvas.width - w - self.px(4.0)), y: self.pointer.1 + self.px(10.0), w, h };
            self.canvas.panel(r, self.px(7.0), ch.panel, self.cur_accent);
            let (tx, ty) = (r.x + self.px(12.0), r.y + self.px(6.0));
            self.canvas.text(&mut self.fonts.side, &t.label, tx as f32, ty, ch.text, r);
        }
        self.damage.push(Rect { x: 0, y: 0, w: self.canvas.width, h: self.canvas.height });
    }
}
