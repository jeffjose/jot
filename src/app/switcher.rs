//! The workspace switcher (S2 on the "jot: workspace and project color"
//! canvas): opened from the workspace's name on the foot bar (up) or its
//! chip in the title bar (down). A row a workspace of the machine the bar
//! shows: its mark on its color, its name and folder, how many panes it
//! has, and at the right its Alt key; under the pointer a pencil (rename it)
//! and a cog (its settings) in that same place, so nothing moves. The one
//! you are in a neutral step lighter; New workspace at the end. Alt 1-9
//! still go straight there.

use super::foot::FootDot;
use super::*;
use crate::canvas::icons;

/// What a click in the switcher is on.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hit {
    Row(usize),
    Rename(usize),
    Settings(usize),
    New,
}

pub struct Switcher {
    /// Up from the foot bar, or down from the title bar.
    pub up: bool,
    /// Its left, and the edge it hangs from (the foot bar's top, the
    /// title bar's bottom).
    pub at: (i32, i32),
    /// The row under the pointer (or picked with the arrows).
    pub hover: Option<usize>,
    pub hits: Vec<(Rect, Hit)>,
    pub rect: Rect,
}

/// A workspace as the switcher lists it.
struct Row {
    dot: FootDot,
    name: String,
    color: u32,
    mark: Option<char>,
    folder: String,
    panes: usize,
    here: bool,
}

impl App {
    fn switcher_rows(&self) -> Vec<Row> {
        let set = self.machine_set(self.foot_machine());
        let names: Vec<&str> = set.iter().map(|(_, n, _)| n.as_str()).collect();
        let marks = super::projects::workspace_marks(&names);
        set.iter()
            .enumerate()
            .map(|(i, (d, name, color))| {
                let (folder, panes) = match *d {
                    FootDot::Here(wi) => {
                        let ws = &self.workspaces[wi];
                        let mut ids = Vec::new();
                        ws.root.panes(&mut ids);
                        ids.retain(|id| !ws.spaces.contains(id));
                        let folder = self.panes.get(&ws.focus).and_then(|p| p.pty.cwd()).map(|c| persist::shorten(&c)).unwrap_or_default();
                        (folder, ids.len())
                    }
                    FootDot::There(..) => {
                        let cwds = self.dot_cwds(*d);
                        (cwds.first().map(|c| persist::shorten(std::path::Path::new(c))).unwrap_or_default(), cwds.len())
                    }
                };
                Row { dot: *d, name: name.clone(), color: *color, mark: marks.get(i).copied(), folder, panes, here: *d == FootDot::Here(self.current) }
            })
            .collect()
    }

    /// The switcher opened (or shut, when it is open already): `up` from a
    /// point on the foot bar's top, else down from the title bar's foot.
    pub(super) fn toggle_switcher(&mut self, up: bool, at: (i32, i32)) {
        if self.switcher.take().is_some() {
            self.full = true;
            return self.redraw();
        }
        self.menu = None;
        self.palette = None;
        let hover = self.switcher_rows().iter().position(|r| r.here);
        self.switcher = Some(Switcher { up, at, hover, hits: Vec::new(), rect: Rect::default() });
        self.full = true;
        self.redraw();
    }

    pub(super) fn close_switcher(&mut self) {
        if self.switcher.take().is_some() {
            self.full = true;
            self.redraw();
        }
    }

    /// Drawn over everything but the tooltip, each frame it is up.
    pub(super) fn draw_switcher(&mut self) {
        let Some(sw) = self.switcher.as_ref() else { return };
        let (up, at, hover) = (sw.up, sw.at, sw.hover);
        let rows = self.switcher_rows();
        let c = self.cur_theme.chrome();
        let scale = self.scale;
        let px = move |v: f32| (v * scale).round() as i32;
        let (uh, sh) = (self.fonts.ui.height, self.fonts.small.height);
        let pad = px(6.0);
        let head_h = sh + px(14.0);
        let row_h = uh + sh + px(18.0);
        let new_h = uh + px(16.0);
        let w = px(380.0).min(self.canvas.width - px(16.0));
        let h = pad + head_h + rows.len() as i32 * (row_h + px(2.0)) + px(9.0) + new_h + pad;
        let x = at.0.clamp(px(4.0), (self.canvas.width - w - px(4.0)).max(0));
        let y = if up { (at.1 - h - px(4.0)).max(px(4.0)) } else { (at.1 + px(4.0)).min(self.canvas.height - h - px(4.0)).max(0) };
        let r = Rect { x, y, w, h };
        let s = px(24.0);
        let area = Rect { x: r.x - s, y: r.y - s, w: r.w + 2 * s, h: r.h + 2 * s };
        self.float_save(area);
        self.canvas.panel(r, px(10.0), c.panel, c.edge);
        let mut hits = Vec::new();
        // Its head: the machine whose workspaces these are, and the keys.
        let machine = self.machine_name(self.foot_machine());
        let hy = r.y + pad + (head_h - sh) / 2;
        self.canvas.text(&mut self.fonts.small, &format!("Workspaces · {machine}"), (r.x + px(14.0)) as f32, hy, c.faint, r);
        let keys = "Alt 1–9";
        let kw = self.fonts.small.width(keys);
        self.canvas.text(&mut self.fonts.small, keys, (r.x + r.w - px(14.0) - kw) as f32, hy, c.faint, r);
        let mut y = r.y + pad + head_h;
        let slot_w = px(56.0);
        for (i, row) in rows.iter().enumerate() {
            let cell = Rect { x: r.x + pad, y, w: r.w - 2 * pad, h: row_h };
            let lit = hover == Some(i);
            // The one you are in: a neutral step lighter (C1 on the canvas),
            // its color left to its mark; under the pointer, a step more.
            if row.here {
                self.canvas.round_fill(cell, px(7.0), mix(c.panel, c.text, if lit { 34 } else { 26 }), 255);
            } else if lit {
                self.canvas.round_fill(cell, px(7.0), mix(c.panel, c.text, 14), 255);
            }
            // Its mark, dark on its color.
            let t = px(30.0);
            let tile = Rect { x: cell.x + px(8.0), y: cell.y + (row_h - t) / 2, w: t, h: t };
            self.canvas.round_fill(tile, px(6.0), row.color, 255);
            if let Some(m) = row.mark {
                let g = px(17.0);
                self.canvas.icon_in(m, Rect { x: tile.x + (t - g) / 2, y: tile.y + (t - g) / 2, w: g, h: g }, g, mix(row.color, 0x000000, 215));
            }
            // At the right a slot always the same width: its Alt key, or
            // under the pointer the pencil and the cog in its place.
            let slot = Rect { x: cell.x + cell.w - px(8.0) - slot_w, y: cell.y, w: slot_w, h: row_h };
            let n = i + 1;
            if lit {
                let b = Rect { x: slot.x + slot.w - px(52.0), y: cell.y + (row_h - px(24.0)) / 2, w: px(25.0), h: px(24.0) };
                for (k, (glyph, hit)) in [(icons::PENCIL, Hit::Rename(i)), (icons::SETTINGS, Hit::Settings(i))].into_iter().enumerate() {
                    let br = Rect { x: b.x + k as i32 * px(27.0), ..b };
                    if self.switcher.as_ref().is_some_and(|s| s.hits.iter().any(|(hr, h)| *h == hit && hr.contains(self.pointer.0, self.pointer.1))) {
                        self.canvas.round_fill(br, px(5.0), mix(c.panel, c.text, 40), 255);
                    }
                    self.canvas.icon_in(glyph, br, px(14.0), c.text);
                    hits.push((br, hit));
                }
            } else if n <= 9 {
                let label = format!("Alt {n}");
                let lw = self.fonts.small.width(&label) + px(12.0);
                let chip = Rect { x: slot.x + slot.w - lw, y: cell.y + (row_h - sh - px(6.0)) / 2, w: lw, h: sh + px(6.0) };
                self.canvas.ring(chip, px(4.0), 1, mix(c.panel, c.text, 50));
                self.canvas.text(&mut self.fonts.small, &label, (chip.x + px(6.0)) as f32, chip.y + px(3.0), c.dim, chip);
            }
            // How many panes it has, before the slot.
            let count = format!("{} pane{}", row.panes, if row.panes == 1 { "" } else { "s" });
            let cw = self.fonts.small.width(&count);
            let cx = slot.x - px(10.0) - cw;
            self.canvas.text(&mut self.fonts.small, &count, cx as f32, cell.y + (row_h - sh) / 2, c.dim, cell);
            // Its name, its folder under it.
            let nx = tile.x + t + px(10.0);
            let room = (cx - px(10.0) - nx).max(0);
            let top = cell.y + (row_h - uh - sh - px(3.0)) / 2;
            self.canvas.text_fit(&mut self.fonts.ui, &row.name, nx, top, room, c.text, cell);
            if !row.folder.is_empty() {
                self.canvas.text_fit(&mut self.fonts.small, &row.folder, nx, top + uh + px(3.0), room, c.dim, cell);
            }
            hits.push((cell, Hit::Row(i)));
            y += row_h + px(2.0);
        }
        // New workspace, after a line.
        y += px(4.0);
        self.canvas.fill(Rect { x: r.x + px(10.0), y, w: r.w - px(20.0), h: 1 }, c.edge);
        y += px(5.0);
        let cell = Rect { x: r.x + pad, y, w: r.w - 2 * pad, h: new_h };
        let new_lit = self.switcher.as_ref().is_some_and(|s| s.hits.iter().any(|(hr, h)| *h == Hit::New && hr.contains(self.pointer.0, self.pointer.1)));
        if new_lit {
            self.canvas.round_fill(cell, px(7.0), mix(c.panel, c.text, 16), 255);
        }
        let ih = px(13.0);
        self.canvas.icon_in(icons::PLUS, Rect { x: cell.x + px(16.0), y: cell.y + (new_h - ih) / 2, w: ih, h: ih }, ih, c.dim);
        self.canvas.text(&mut self.fonts.ui, "New workspace", (cell.x + px(16.0) + ih + px(12.0)) as f32, cell.y + (new_h - uh) / 2, c.text, cell);
        let kw = self.fonts.small.width("Ctrl N");
        self.canvas.text(&mut self.fonts.small, "Ctrl N", (cell.x + cell.w - px(12.0) - kw) as f32, cell.y + (new_h - sh) / 2, c.faint, cell);
        hits.push((cell, Hit::New));
        if let Some(sw) = self.switcher.as_mut() {
            sw.hits = hits;
            sw.rect = r;
        }
        self.damage.push(area);
    }

    /// What in the switcher is at a point: a row's pencil or cog before
    /// the row itself.
    fn switcher_hit(&self, x: i32, y: i32) -> Option<Hit> {
        let sw = self.switcher.as_ref()?;
        let at: Vec<Hit> = sw.hits.iter().filter(|(r, _)| r.contains(x, y)).map(|(_, h)| *h).collect();
        at.iter().find(|h| matches!(h, Hit::Rename(_) | Hit::Settings(_))).or(at.first()).copied()
    }

    /// The pointer over the window while the switcher is up: the row under
    /// it lit (its pencil and cog shown), the hand on what a click does.
    pub(super) fn switcher_hover(&mut self, x: i32, y: i32) {
        let hit = self.switcher_hit(x, y);
        let row = match hit {
            Some(Hit::Row(i) | Hit::Rename(i) | Hit::Settings(i)) => Some(i),
            _ => None,
        };
        if let Some(sw) = self.switcher.as_mut() {
            if row.is_some() {
                sw.hover = row;
            }
        }
        if let Some(w) = &self.window {
            w.set_cursor(if hit.is_some() { CursorIcon::Pointer } else { CursorIcon::Default });
        }
        // Drawn again each move: a lit pencil, cog or row follows it.
        self.redraw();
    }

    /// A click while the switcher is up: what it is on done, the switcher
    /// shut; outside it, only shut.
    pub(super) fn switcher_click(&mut self) {
        let (x, y) = self.pointer;
        let hit = self.switcher_hit(x, y);
        let rows = self.switcher_rows();
        self.close_switcher();
        match hit {
            Some(Hit::Row(i)) => {
                if let Some(r) = rows.get(i) {
                    self.go_dot(r.dot);
                }
            }
            Some(Hit::Rename(i)) => {
                if let Some(r) = rows.get(i) {
                    self.go_dot(r.dot);
                    if matches!(r.dot, FootDot::Here(_)) {
                        self.rename_workspace();
                    }
                }
            }
            Some(Hit::Settings(_)) => {
                self.open_settings();
                if let Some(s) = &mut self.settings {
                    s.section = 1;
                }
            }
            Some(Hit::New) => self.new_workspace(),
            None => {}
        }
    }

    /// A key while the switcher is up: the arrows pick a row, Enter goes
    /// there, Escape shuts it; true when it was the switcher's.
    pub(super) fn switcher_key(&mut self, code: KeyCode) -> bool {
        let n = self.switcher_rows().len();
        let Some(sw) = self.switcher.as_mut() else { return false };
        match code {
            KeyCode::Escape => self.close_switcher(),
            KeyCode::ArrowDown | KeyCode::ArrowUp if n > 0 => {
                let now = sw.hover.unwrap_or(0);
                sw.hover = Some(if code == KeyCode::ArrowDown { (now + 1) % n } else { (now + n - 1) % n });
                self.redraw();
            }
            KeyCode::Enter | KeyCode::NumpadEnter => {
                let pick = sw.hover;
                let rows = self.switcher_rows();
                self.close_switcher();
                if let Some(r) = pick.and_then(|i| rows.get(i)) {
                    self.go_dot(r.dot);
                }
            }
            _ => return false,
        }
        true
    }
}
