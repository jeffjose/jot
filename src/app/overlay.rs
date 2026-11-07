//! What floats over the panes: the palette (Cmd-K) and the right-click
//! menu, both drawn as jog draws its own, a panel with the pick filled in.

use super::*;
use crate::canvas::on;
use crate::field::{Outcome, TextField};
use crate::palette::{self, Action, Item, Mode, Palette};


/// The right-click menu: items, with lines between groups (None).
pub struct Menu {
    pub items: Vec<Option<Item>>,
    pub at: (i32, i32),
    pub hover: Option<usize>,
}

pub struct Layout {
    pub panel: Rect,
    pub pad: i32,
    pub head: i32,
    pub row_h: i32,
    pub rows: usize,
}

impl App {
    fn recent_path() -> PathBuf {
        std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| pty::home().join(".local/state")).join("jterm/palette-recent")
    }

    pub(super) fn load_recent(&mut self) {
        let text = std::fs::read_to_string(Self::recent_path()).unwrap_or_default();
        self.recent = text.lines().filter(|l| !l.is_empty()).map(str::to_string).collect();
    }

    fn remember(&mut self, label: &str) {
        self.recent.retain(|l| l != label);
        self.recent.insert(0, label.to_string());
        self.recent.truncate(30);
        let path = Self::recent_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, self.recent.join("\n"));
    }

    /// Everything the palette can find: the workspaces, the commands, and
    /// the project folders not open as a workspace already.
    fn palette_items(&self) -> Vec<Item> {
        let mut items: Vec<Item> = self
            .workspaces
            .iter()
            .enumerate()
            .map(|(i, ws)| Item {
                label: ws.name.clone(),
                hint: if i == self.current { "workspace, here".into() } else { format!("workspace · Alt {}", i + 1) },
                action: Action::GoTo(i),
            })
            .collect();
        // Every pane, by its caption, in every workspace.
        for (wi, ws) in self.workspaces.iter().enumerate() {
            let mut ids = Vec::new();
            ws.root.panes(&mut ids);
            for (n, id) in ids.iter().enumerate() {
                if let Some(p) = self.panes.get(id) {
                    items.push(Item { label: p.caption.clone(), hint: format!("{} · pane {}", ws.name, n + 1), action: Action::Pane(wi, *id) });
                }
            }
        }
        items.extend(palette::commands());
        let roots: Vec<PathBuf> = self.config.projects.iter().map(|p| persist::expand(p)).collect();
        let open: Vec<String> = self.workspaces.iter().map(|w| w.name.clone()).collect();
        for dir in palette::projects(&roots) {
            let name = folder_name(&dir);
            if open.contains(&name) {
                continue;
            }
            let hint = dir.parent().map(|p| persist::shorten(p)).unwrap_or_default();
            items.push(Item { label: name, hint, action: Action::Open(dir) });
        }
        items
    }

    pub(super) fn open_palette(&mut self) {
        self.open_palette_with("");
    }

    /// The palette with something already typed: `>` for the commands,
    /// on Ctrl P, as jog has it.
    pub(super) fn open_palette_with(&mut self, typed: &str) {
        self.menu = None;
        self.settings = None;
        let mut p = Palette::new();
        p.field = TextField::new(typed);
        self.palette = Some(p);
        self.palette_changed();
    }

    fn palette_changed(&mut self) {
        let Some(p) = &self.palette else { return };
        if p.mode == Mode::Find {
            let matches = palette::matching(self.palette_items(), &p.typed(), &self.recent);
            if let Some(p) = &mut self.palette {
                p.matches = matches;
                p.selected = 0;
                p.first = 0;
            }
        }
        self.full = true;
        self.redraw();
    }

    pub(super) fn close_overlay(&mut self) {
        if self.palette.take().is_some() | self.menu.take().is_some() {
            self.full = true;
            self.redraw();
        }
    }

    /// A key while the palette is open: all keys go to it.
    pub(super) fn palette_key(&mut self, event: &winit::event::KeyEvent, event_loop: &ActiveEventLoop) {
        let PhysicalKey::Code(code) = event.physical_key else { return };
        let rows = self.palette_layout().map_or(1, |l| l.rows);
        let mods = self.mods;
        let ctrl = mods.control_key();
        // The clipboard, for Ctrl C, X and V in what is typed.
        let pasted = if ctrl && code == KeyCode::KeyV { self.clip_get(false) } else { None };
        let copied: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
        let Some(p) = &mut self.palette else { return };
        let outcome = match code {
            KeyCode::Escape => return self.close_overlay(),
            KeyCode::Enter | KeyCode::NumpadEnter => {
                return self.palette_run(event_loop);
            }
            KeyCode::ArrowUp => {
                p.step(-1, rows);
                Outcome::Moved
            }
            KeyCode::ArrowDown | KeyCode::Tab => {
                p.step(1, rows);
                Outcome::Moved
            }
            KeyCode::KeyP if ctrl => {
                p.step(-1, rows);
                Outcome::Moved
            }
            KeyCode::KeyN if ctrl => {
                p.step(1, rows);
                Outcome::Moved
            }
            KeyCode::PageUp => {
                p.step(-(rows as isize), rows);
                Outcome::Moved
            }
            KeyCode::PageDown => {
                p.step(rows as isize, rows);
                Outcome::Moved
            }
            _ => {
                let copied = &copied;
                p.field.key(code, mods, event.text.as_deref(), &mut || pasted.clone(), &mut |s| *copied.borrow_mut() = Some(s))
            }
        };
        if let Some(s) = copied.into_inner() {
            self.clip_set(s, false);
        }
        match outcome {
            Outcome::Changed => self.palette_changed(),
            Outcome::Moved => {
                self.full = true;
                self.redraw();
            }
            Outcome::Ignored => {}
        }
    }

    fn palette_run(&mut self, event_loop: &ActiveEventLoop) {
        let Some(p) = self.palette.take() else { return };
        self.full = true;
        if p.mode == Mode::Rename {
            let name = p.typed().trim().to_string();
            if !name.is_empty() {
                self.ws_mut().name = name;
                self.unsaved = true;
                self.set_title();
            }
            return self.redraw();
        }
        let Some(item) = p.matches.get(p.selected).cloned() else { return self.redraw() };
        self.remember(&item.label);
        self.run(item.action, event_loop);
    }

    /// Does what the palette or the menu picked.
    pub(super) fn run(&mut self, action: Action, event_loop: &ActiveEventLoop) {
        self.full = true;
        match action {
            Action::Split(dir) => self.split(dir),
            Action::Close => self.close_pane(event_loop),
            Action::NewPane => self.new_pane(),
            Action::Reopen => self.reopen(),
            Action::Zoom => {
                if !self.roam_shown {
                    let ws = self.ws_mut();
                    ws.zoom = if ws.zoom.is_some() { None } else { Some(ws.focus) };
                    self.changed();
                }
            }
            Action::Equalize => {
                self.ws_mut().root.equalize();
                self.changed();
            }
            Action::Only => self.only(),
            Action::GoTo(i) => self.go_to(i),
            Action::Pane(wi, id) => {
                self.go_to(wi);
                self.roam_shown = false;
                if let Some(ws) = self.workspaces.get_mut(wi) {
                    if ws.zoom.is_some_and(|z| z != id) {
                        ws.zoom = None;
                    }
                }
                self.set_focus(id);
                self.relayout();
            }
            Action::NewWorkspace => self.new_workspace(),
            Action::Open(dir) => {
                self.add_workspace(&WorkspaceSpec { name: String::new(), cwd: Some(dir.display().to_string()), panes: vec![], split: None, layout: None, ..Default::default() });
                self.go_to(self.workspaces.len() - 1);
            }
            Action::Rename => {
                let name = self.ws().name.clone();
                // The name is all selected, to type over or to change.
                let mut field = TextField::new(&name);
                field.anchor = Some(0);
                self.palette = Some(Palette { mode: Mode::Rename, field, matches: Vec::new(), selected: 0, first: 0 });
            }
            Action::CloseWorkspace => {
                let mut ids = Vec::new();
                self.ws().root.panes(&mut ids);
                for id in ids {
                    self.close(id, event_loop);
                }
            }
            Action::Roam => self.toggle_roam(),
            Action::Clear => {
                if let Some(p) = self.focused().and_then(|f| self.panes.get_mut(&f)) {
                    p.term.clear();
                    // The shell draws its prompt again at the top.
                    p.pty.write(b"\x0c");
                    p.full = true;
                }
            }
            Action::Copy => self.copy(false),
            Action::Paste => self.paste(false),
            Action::FontLarger => self.set_font_size(self.font_size + 1.0),
            Action::FontSmaller => self.set_font_size(self.font_size - 1.0),
            Action::FontReset => self.set_font_size(self.config.font_size),
            Action::Settings => self.open_settings(),
            Action::Broadcast => self.toggle_broadcast(),
            Action::PopOut => self.pop_out(event_loop),
            Action::CopyOutput => self.copy_output_at(self.menu_at),
            Action::Palette => self.open_palette(),
            Action::Commands => self.open_palette_with(">"),
            Action::StatusLine => {
                self.config.status_line = !self.config.status_line;
                self.relayout();
            }
        }
        self.redraw();
    }

    pub(super) fn palette_layout(&self) -> Option<Layout> {
        let p = self.palette.as_ref()?;
        let (w, h) = (self.canvas.width, self.canvas.height);
        let line = self.fonts.ui.height;
        let pad = self.px(12.0);
        let (head, row_h) = (line + 2 * pad, line * 2);
        let fit = ((h * 3 / 4 - head) / row_h).clamp(1, 10) as usize;
        let rows = if p.mode == Mode::Rename { 0 } else { fit.min(p.matches.len()) };
        let width = (self.px(14.0) * 42).min(w - 2 * self.px(16.0));
        let height = head + rows as i32 * row_h + if rows > 0 { pad / 2 } else { 0 };
        // A fifth of the way down, where it stays as matches come and go.
        let top = (h - head - fit as i32 * row_h) / 5;
        Some(Layout { panel: Rect { x: (w - width) / 2, y: top.max(pad), w: width, h: height }, pad, head, row_h, rows })
    }

    pub(super) fn draw_palette(&mut self) {
        let c = self.cur_theme.chrome();
        let Some(l) = self.palette_layout() else { return };
        let Some(p) = self.palette.as_ref() else { return };
        let (label, field, mode) = (p.label(), p.field.clone(), p.mode);
        let note = match (mode, p.matches.len()) {
            (Mode::Rename, _) => "Enter to name it".to_string(),
            (_, 0) => "no match".to_string(),
            (_, n) => format!("{} of {n}", p.selected + 1),
        };
        let shown: Vec<(usize, Item)> = p.matches.iter().cloned().enumerate().skip(p.first).take(l.rows).collect();
        let selected = p.selected;
        let r = l.panel;
        self.canvas.panel(r, l.pad * 2 / 3, c.panel, c.edge);
        let ui = &mut self.fonts.ui;
        let (x, y) = (r.x + l.pad, r.y + l.pad);
        let after_label = self.canvas.text(ui, label, x as f32, y, c.dim, r);
        let typed_x = after_label + l.pad;
        let selection = mix(self.cur_accent, c.panel, 140);
        field.draw(&mut self.canvas, ui, typed_x, y, c.text, selection, true, r);
        let note_w = ui.width(&note);
        self.canvas.text(ui, &note, (r.x + r.w - l.pad - note_w) as f32, y, c.dim, r);
        for (row, (at, item)) in shown.iter().enumerate() {
            let top = r.y + l.head + row as i32 * l.row_h;
            let picked = *at == selected;
            if picked {
                self.canvas.round_fill(Rect { x: r.x + l.pad / 2, y: top + 1, w: r.w - l.pad, h: l.row_h - 2 }, l.pad * 3 / 4, self.cur_accent, 255);
            }
            let (main, dim) = if picked { (on(self.cur_accent), mix(self.cur_accent, on(self.cur_accent), 200)) } else { (c.text, c.dim) };
            let ty = top + (l.row_h - ui.height) / 2;
            let row_clip = Rect { x: r.x, y: top, w: r.w, h: l.row_h };
            self.canvas.text(ui, &item.label, (r.x + l.pad * 3 / 2) as f32, ty, main, row_clip);
            let hint_w = ui.width(&item.hint);
            self.canvas.text(ui, &item.hint, (r.x + r.w - l.pad * 3 / 2 - hint_w) as f32, ty, dim, row_clip);
        }
        self.damage.push(Rect { x: 0, y: 0, w: self.canvas.width, h: self.canvas.height });
    }

    fn palette_row_at(&self, x: i32, y: i32) -> Option<usize> {
        let l = self.palette_layout()?;
        let p = self.palette.as_ref()?;
        let row = (y - l.panel.y - l.head).div_euclid(l.row_h);
        (l.panel.contains(x, y) && (0..l.rows as i32).contains(&row)).then(|| p.first + row as usize)
    }

    /// The right-click menu, at the pointer.
    pub(super) fn open_menu(&mut self) {
        let c = |label: &str, hint: &str, action| Some(Item { label: label.into(), hint: hint.into(), action });
        let has_selection = self.focused().and_then(|f| self.panes.get(&f)).is_some_and(|p| p.term.selection_text().is_some());
        let mut items = Vec::new();
        let marked = self.focused().and_then(|f| self.panes.get(&f)).is_some_and(|p| p.shell.marked);
        if has_selection {
            items.push(c("Copy", "Ctrl Shift C", Action::Copy));
        }
        items.extend([
            c("Paste", "Ctrl Shift V", Action::Paste),
            if marked { c("Copy command output", "", Action::CopyOutput) } else { None },
            None,
            c("Split side by side", "Alt V", Action::Split(Dir::Across)),
            c("Split one above another", "Alt S", Action::Split(Dir::Down)),
            c(if self.ws().zoom.is_some() { "Show all panes" } else { "Pane alone" }, "Alt Shift M", Action::Zoom),
            c("New pane", "Ctrl T", Action::NewPane),
            c("Pop out", "Alt Shift O", Action::PopOut),
            c("Close pane", "Ctrl W", Action::Close),
            c("Reopen closed pane", "Ctrl Shift T", Action::Reopen),
            None,
            c("Clear screen and scrollback", "", Action::Clear),
            c("Roaming terminal", "Alt `", Action::Roam),
            None,
            c("New workspace here", "Alt Shift N", Action::NewWorkspace),
            c("Rename workspace…", "", Action::Rename),
            None,
            c("Find…", "Ctrl K", Action::Palette),
            c("Run a command…", "Ctrl P", Action::Commands),
            c("Settings…", "Ctrl ,", Action::Settings),
        ]);
        self.palette = None;
        let items: Vec<Option<Item>> = items.into_iter().enumerate().filter(|(i, it)| it.is_some() || *i > 0).map(|(_, it)| it).collect();
        self.menu_at = self.pointer;
        self.menu = Some(Menu { items, at: self.pointer, hover: None });
        self.full = true;
        self.redraw();
    }

    pub(super) fn menu_layout(&mut self) -> Option<(Rect, i32, i32, i32)> {
        let m = self.menu.as_ref()?;
        let ui = &mut self.fonts.ui;
        let pad = (self.scale * 8.0).round() as i32;
        let row_h = ui.height + pad;
        let sep_h = pad;
        let widest = m.items.iter().flatten().map(|i| ui.width(&i.label) + if i.hint.is_empty() { 0 } else { ui.width(&i.hint) + 3 * pad }).max().unwrap_or(0);
        let w = widest + 4 * pad;
        let h = m.items.iter().map(|i| if i.is_some() { row_h } else { sep_h }).sum::<i32>() + pad;
        // Kept inside the window, flipped to the pointer's other side when
        // there is no room.
        let (mut x, mut y) = m.at;
        if x + w > self.canvas.width {
            x = (x - w).max(0);
        }
        if y + h > self.canvas.height {
            y = (y - h).max(0);
        }
        Some((Rect { x, y, w, h }, pad, row_h, sep_h))
    }

    /// Which item of the menu is at a point.
    pub(super) fn menu_item_at(&mut self, x: i32, y: i32) -> Option<usize> {
        let (r, pad, row_h, sep_h) = self.menu_layout()?;
        if !r.contains(x, y) {
            return None;
        }
        let m = self.menu.as_ref()?;
        let mut top = r.y + pad / 2;
        for (i, item) in m.items.iter().enumerate() {
            let h = if item.is_some() { row_h } else { sep_h };
            if y >= top && y < top + h {
                return item.is_some().then_some(i);
            }
            top += h;
        }
        None
    }

    pub(super) fn draw_menu(&mut self) {
        let c = self.cur_theme.chrome();
        let Some((r, pad, row_h, sep_h)) = self.menu_layout() else { return };
        let Some(m) = self.menu.as_ref() else { return };
        let (items, hover) = (m.items.clone(), m.hover);
        self.canvas.panel(r, pad, c.panel, c.edge);
        let ui = &mut self.fonts.ui;
        let mut top = r.y + pad / 2;
        for (i, item) in items.iter().enumerate() {
            let Some(item) = item else {
                self.canvas.fill(Rect { x: r.x + pad, y: top + sep_h / 2, w: r.w - 2 * pad, h: 1 }, c.edge);
                top += sep_h;
                continue;
            };
            let picked = hover == Some(i);
            if picked {
                self.canvas.round_fill(Rect { x: r.x + pad / 2, y: top, w: r.w - pad, h: row_h }, pad / 2, self.cur_accent, 255);
            }
            let (main, dim) = if picked { (on(self.cur_accent), mix(self.cur_accent, on(self.cur_accent), 200)) } else { (c.text, c.faint) };
            let ty = top + (row_h - ui.height) / 2;
            self.canvas.text(ui, &item.label, (r.x + pad * 3 / 2) as f32, ty, main, r);
            if !item.hint.is_empty() {
                let hw = ui.width(&item.hint);
                self.canvas.text(ui, &item.hint, (r.x + r.w - pad * 3 / 2 - hw) as f32, ty, dim, r);
            }
            top += row_h;
        }
        self.damage.push(Rect { x: 0, y: 0, w: self.canvas.width, h: self.canvas.height });
    }

    /// A click while something floats: the palette or the menu gets it.
    /// True when it was theirs.
    pub(super) fn overlay_click(&mut self, button: MouseButton, event_loop: &ActiveEventLoop) -> bool {
        let (x, y) = self.pointer;
        if self.menu.is_some() {
            let hit = self.menu_item_at(x, y);
            let item = hit.and_then(|i| self.menu.as_ref().and_then(|m| m.items[i].clone()));
            self.close_overlay();
            if let (Some(item), MouseButton::Left) = (item, button) {
                self.run(item.action, event_loop);
            }
            return true;
        }
        if self.palette.is_some() {
            match self.palette_row_at(x, y) {
                Some(row) if button == MouseButton::Left => {
                    if let Some(p) = &mut self.palette {
                        p.selected = row;
                    }
                    self.palette_run(event_loop);
                }
                _ if self.palette_layout().is_some_and(|l| l.panel.contains(x, y)) => {}
                _ => self.close_overlay(),
            }
            return true;
        }
        false
    }

    /// The pointer over the menu or the palette: the row under it picked.
    pub(super) fn overlay_hover(&mut self, x: i32, y: i32) -> bool {
        if self.menu.is_some() {
            let hit = self.menu_item_at(x, y);
            if let Some(m) = &mut self.menu {
                if m.hover != hit {
                    m.hover = hit;
                    self.full = true;
                    self.redraw();
                }
            }
            return true;
        }
        if self.palette.is_some() {
            if let Some(row) = self.palette_row_at(x, y) {
                if let Some(p) = &mut self.palette {
                    if p.selected != row {
                        p.selected = row;
                        self.full = true;
                        self.redraw();
                    }
                }
            }
            return true;
        }
        false
    }

    pub(super) fn set_title(&self) {
        if let (Some(w), Some(ws)) = (&self.window, self.workspaces.get(self.current)) {
            w.set_title(&format!("{} · jterm", ws.name));
        }
    }
}
