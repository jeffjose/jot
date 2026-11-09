//! What floats over the panes: the palette (Cmd-K) and the right-click
//! menu, both drawn as jog draws its own, a panel with the pick filled in.

use super::*;
use crate::canvas::on;
use crate::field::{Outcome, TextField};
use crate::palette::{self, Action, Item, Mode, Palette};


/// The right-click menu: items, with lines between groups (None). Rows
/// whose action is Submenu(n) open `children[n]` beside them.
pub struct Menu {
    pub items: Vec<Option<Item>>,
    pub at: (i32, i32),
    pub hover: Option<usize>,
    pub children: Vec<Vec<Option<Item>>>,
    /// The submenu shown: the row it hangs from, and the row hovered in it.
    pub open: Option<(usize, Option<usize>)>,
}

/// Where a menu, or a submenu, is drawn.
#[derive(Clone, Copy)]
pub struct MenuBox {
    pub rect: Rect,
    pub pad: i32,
    pub row_h: i32,
    pub sep_h: i32,
}

impl MenuBox {
    /// The top of each row.
    fn tops(&self, items: &[Option<Item>]) -> Vec<i32> {
        let mut top = self.rect.y + self.pad / 2;
        items
            .iter()
            .map(|i| {
                let t = top;
                top += if i.is_some() { self.row_h } else { self.sep_h };
                t
            })
            .collect()
    }

    /// The item row at a point, not counting lines between groups.
    fn row_at(&self, items: &[Option<Item>], x: i32, y: i32) -> Option<usize> {
        if !self.rect.contains(x, y) {
            return None;
        }
        let tops = self.tops(items);
        (0..items.len()).find(|&i| items[i].is_some() && y >= tops[i] && y < tops[i] + self.row_h)
    }
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
        std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| pty::home().join(".local/state")).join("jot/palette-recent")
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
                hint: if i == self.current { "current workspace".into() } else { format!("workspace · Alt {}", i + 1) },
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
        // Commands with a state named by what they do now: Start or Stop.
        items.extend(palette::commands().into_iter().map(|mut i| {
            if let Some(label) = self.state_label(&i.action) {
                i.label = label.into();
            }
            i
        }));
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
        let Some(mode) = self.palette.as_ref().map(|p| p.mode) else { return };
        if mode == Mode::Machine || mode == Mode::AddMachine {
            let typed = self.palette.as_ref().map(|p| p.typed()).unwrap_or_default();
            let matches = machine_items(&typed, mode == Mode::AddMachine);
            if let Some(p) = &mut self.palette {
                p.matches = matches;
                p.selected = 0;
                p.first = 0;
            }
        }
        if mode == Mode::Find {
            let typed = self.palette.as_ref().map(|p| p.typed()).unwrap_or_default();
            let matches = palette::matching(self.palette_items(), &typed, &self.recent);
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
        if let Mode::RenameTerminal(k, sid) = p.mode {
            self.name_terminal(k, sid, &p.typed());
            return self.redraw();
        }
        if p.mode == Mode::Rename {
            let name = p.typed().trim().to_string();
            if !name.is_empty() {
                self.ws_mut().name = name;
                self.unsaved = true;
                self.set_title();
            }
            return self.redraw();
        }
        // A machine as typed, when none listed is picked.
        if (p.mode == Mode::Machine || p.mode == Mode::AddMachine) && p.matches.is_empty() {
            let host = p.typed().trim().to_string();
            if !host.is_empty() {
                self.run(if p.mode == Mode::AddMachine { Action::AddMachine(host) } else { Action::Connect(host) }, event_loop);
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
            Action::SplitBefore(dir) => self.split_side(dir, true),
            Action::Close => self.close_pane(event_loop),
            Action::EndPane => self.kill_terminal(false, event_loop),
            Action::KillTerminalNow => self.kill_terminal(true, event_loop),
            Action::ShowTerminal(k, sid) => {
                if let Some(at) = self.focused() {
                    self.take_terminal(k, sid, terminals::Place::Into(at));
                }
            }
            Action::PickTerminal => self.terminal_picker(),
            Action::NewWindow => self.new_window(),
            Action::NewTerminalHere => self.new_terminal_here(),
            Action::NextTerminal => self.step_terminal(1),
            Action::PrevTerminal => self.step_terminal(-1),
            Action::SplitTerminal(k, sid) => {
                if let Some(at) = self.focused() {
                    self.take_terminal(k, sid, terminals::Place::Split(at, Dir::Across, false));
                }
            }
            Action::OpenInNewWindow(k, sid) => self.open_in_new_window(k, sid),
            Action::KillFree(k, sid) => self.kill_free(k, sid),
            Action::RenameTerminal(k, sid) => self.rename_terminal(k, sid),
            Action::NewTerminalOn(k) => self.new_terminal_on(k),
            Action::Reconnect(k) => {
                if let Some(m) = self.machines.get(k) {
                    m.client.try_again();
                }
            }
            Action::KillFreeOn(k) => {
                for f in self.free_terminals().into_iter().filter(|f| f.k == k && !f.elsewhere) {
                    self.kill_free(f.k, f.sid);
                }
            }
            Action::RemoveMachine(k) => self.remove_machine(k),
            Action::QuitAll => {
                // Every pane ended, not left in the session.
                self.save_scrollback();
                self.popouts.clear();
                for p in self.panes.values_mut() {
                    p.pty.end = true;
                }
                self.panes.clear();
                event_loop.exit();
            }
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
            Action::Sidebar => self.toggle_sidebar(),
            Action::ConnectPrompt => {
                self.palette = Some(Palette { mode: Mode::Machine, field: TextField::default(), matches: Vec::new(), selected: 0, first: 0 });
                self.palette_changed();
            }
            Action::Connect(host) => self.connect_pane(&host),
            Action::AddMachine(host) => self.add_machine(&host),
            Action::Rename if self.rename_workspace() => {}
            Action::Rename => {
                let name = self.ws().name.clone();
                // The name is all selected, to type over or to change.
                let mut field = TextField::new(&name);
                field.anchor = Some(0);
                self.palette = Some(Palette { mode: Mode::Rename, field, matches: Vec::new(), selected: 0, first: 0 });
            }
            Action::ShowMachine(k) => self.show_machine(k),
            Action::WsAccent(name) => {
                let cur = self.current;
                if let Some(ws) = self.workspaces.get_mut(cur) {
                    ws.accent = name;
                }
                self.unsaved = true;
                self.retint();
                self.full = true;
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
            Action::CopyCommand => self.copy_command_at(self.menu_at),
            Action::Rerun => self.rerun_at(self.menu_at),
            Action::Pin => {
                if let (Some(cmd), Some((id, _))) = (self.command_at(self.menu_at), self.pane_at(self.menu_at.0, self.menu_at.1)) {
                    self.pin_add(id, &cmd);
                }
            }
            Action::PinRun(id, i) => self.pin_run(id, i),
            Action::MiniRestore(id) => self.restore(id),
            Action::MiniFocus(id) => {
                self.restore(id);
                let ws = self.ws_mut();
                ws.zoom = Some(id);
                self.changed();
            }
            Action::MiniRestart(id) => self.restart(id),
            Action::MiniStop(id) => self.stop_job(id),
            Action::MiniCopyLine(id) => {
                if let Some(line) = self.panes.get(&id).map(|p| p.last_line.clone()) {
                    self.clip_set(line, false);
                }
            }
            Action::MiniClose(id) => self.close(id, event_loop),
            Action::MakeSpace => {
                if let Some((id, _)) = self.pane_at(self.menu_at.0, self.menu_at.1).or_else(|| self.focused().map(|f| (f, (0, 0)))) {
                    self.make_space(id);
                }
            }
            Action::SpaceToTerminal(id) => self.space_to_terminal(id),
            Action::SpaceRemove(id) => self.remove_space(id),
            Action::PinRemove(id, i) => self.pin_remove(id, i),
            Action::PinEdit(id, i) => {
                if let Some(cmd) = self.panes.get(&id).and_then(|p| p.pins.get(i)).map(|pin| pin.cmd.clone()) {
                    let busy = !self.pane_waiting(id);
                    self.open_editor_with(id, &cmd, busy);
                }
            }
            Action::EditCommand => self.edit_command_at(self.menu_at),
            Action::Restart => {
                if let Some((id, _)) = self.pane_at(self.menu_at.0, self.menu_at.1) {
                    self.restart(id);
                }
            }
            Action::CommandLine => self.toggle_bar(),
            Action::Minimize => {
                if let Some(id) = self.focused() {
                    self.minimize(id);
                }
            }
            Action::MultiSkip(id) => self.toggle_multi_pane(id),
            Action::MultiAll => {
                if let Some(ws) = self.workspaces.get_mut(self.current) {
                    ws.left_out.clear();
                }
                self.refresh_panes();
            }
            Action::TeeStop(id) => self.stop_tee(id),
            Action::TeeOpen(id) => {
                if let Some(path) = self.panes.get(&id).and_then(|p| p.tee.as_ref()).map(|t| t.path.clone()) {
                    automate::run_detached(&format!("xdg-open '{}'", path.display().to_string().replace('\'', "")), &pty::home(), &[]);
                }
            }
            Action::TeeCopyPath(id) => {
                if let Some(path) = self.panes.get(&id).and_then(|p| p.tee.as_ref()).map(|t| t.path.display().to_string()) {
                    self.clip_set(path, false);
                }
            }
            Action::FilterStop(id) => self.stop_filter(id),
            Action::NotifyOff(id) => self.set_notify(id, false),
            Action::FieldCut | Action::FieldCopy | Action::FieldPaste | Action::FieldSelectAll if self.editor().is_some() => self.editor_menu_do(&action),
            Action::FieldCut | Action::FieldCopy | Action::FieldPaste | Action::FieldSelectAll => self.bar_field_do(&action),
            Action::Palette => self.open_palette(),
            Action::Submenu(_) => {}
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
        let rows = if matches!(p.mode, Mode::Rename | Mode::RenameTerminal(..)) { 0 } else { fit.min(p.matches.len()) };
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
            (Mode::Rename | Mode::RenameTerminal(..), _) => "Enter to rename (empty: its own name again)".to_string(),
            (Mode::Machine | Mode::AddMachine, 0) => "Enter to connect".to_string(),
            (_, 0) => "No matches".to_string(),
            (_, n) => format!("{} of {n}", p.selected + 1),
        };
        let shown: Vec<(usize, Item)> = p.matches.iter().cloned().enumerate().skip(p.first).take(l.rows).collect();
        let selected = p.selected;
        let r = l.panel;
        let m = self.px(24.0);
        let area = Rect { x: r.x - m, y: r.y - m, w: r.w + 2 * m, h: r.h + 2 * m };
        self.float_save(area);
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
        self.damage.push(area);
    }

    fn palette_row_at(&self, x: i32, y: i32) -> Option<usize> {
        let l = self.palette_layout()?;
        let p = self.palette.as_ref()?;
        let row = (y - l.panel.y - l.head).div_euclid(l.row_h);
        (l.panel.contains(x, y) && (0..l.rows as i32).contains(&row)).then(|| p.first + row as usize)
    }

    /// The right-click menu, at the pointer: what is used most on top,
    /// the rest in Pane and Workspace. Everything is in Ctrl P as well.
    pub(super) fn open_menu(&mut self) {
        let c = |label: &str, hint: &str, action| Some(Item { label: label.into(), hint: hint.into(), action });
        let pane = self.focused().and_then(|f| self.panes.get(&f));
        let has_selection = pane.is_some_and(|p| p.term.selection_text().is_some());
        let has_command = self.command_at(self.pointer).is_some();
        // The pane under the pointer running a command: it can be restarted.
        let busy = self.pane_at(self.pointer.0, self.pointer.1).and_then(|(id, _)| self.panes.get(&id)).is_some_and(|p| p.shell.marked && !p.shell.at_prompt && p.shell.running.is_some());
        let mut items = Vec::new();
        if has_selection {
            items.push(c("Copy", "Ctrl Shift C", Action::Copy));
        }
        items.extend([
            c("Paste", "Ctrl Shift V", Action::Paste),
            if has_command { c("Copy output", "", Action::CopyOutput) } else { None },
            if has_command { c("Copy command", "", Action::CopyCommand) } else { None },
            None,
            if busy { c("Restart", "Ctrl R", Action::Restart) } else { None },
            if has_command && !busy { c("Rerun", "", Action::Rerun) } else { None },
            if has_command { c("Edit command", "", Action::EditCommand) } else { None },
            if has_command { c("Pin", "", Action::Pin) } else { None },
            None,
            c("Split right", "Alt V", Action::Split(Dir::Across)),
            c("Split down", "Alt S", Action::Split(Dir::Down)),
            c("Close pane", "Ctrl W", Action::Close),
            None,
            c("Pane", "›", Action::Submenu(0)),
            c("Workspace", "›", Action::Submenu(1)),
            None,
            c("Settings", "Ctrl ,", Action::Settings),
        ]);
        let children = vec![
            vec![
                c(self.state_label(&Action::Zoom).unwrap_or_default(), "Alt Shift F", Action::Zoom),
                c("Minimize", "", Action::Minimize),
                c("Make empty space", "", Action::MakeSpace),
                c("Show another terminal here", "Alt '", Action::PickTerminal),
                c("Kill terminal", "", Action::EndPane),
                c("Split left", "Alt Shift V", Action::SplitBefore(Dir::Across)),
                c("Split up", "Alt Shift S", Action::SplitBefore(Dir::Down)),
                c("New pane", "Ctrl T", Action::NewPane),
                c("Reopen closed pane", "Ctrl Shift T", Action::Reopen),
                c("Pop out", "Alt Shift O", Action::PopOut),
                c("Command line", "Alt ;", Action::CommandLine),
                c(self.state_label(&Action::Broadcast).unwrap_or_default(), "Alt Shift M", Action::Broadcast),
                None,
                c("Clear terminal", "", Action::Clear),
            ],
            vec![
                c("New workspace", "Ctrl N", Action::NewWorkspace),
                c("Rename workspace…", "", Action::Rename),
                None,
                c("Toggle roaming terminal", "Alt `", Action::Roam),
            ],
        ];
        self.palette = None;
        // No line where the menu starts, or two in a row.
        let mut tidy: Vec<Option<Item>> = Vec::new();
        for it in items {
            if it.is_some() || tidy.last().is_some_and(|l| l.is_some()) {
                tidy.push(it);
            }
        }
        self.menu_at = self.pointer;
        self.menu = Some(Menu { items: tidy, at: self.pointer, hover: None, children, open: None });
        self.full = true;
        self.redraw();
    }

    /// Where a list of items goes: at a point, kept inside the window; a
    /// submenu beside its parent, on the left when there is no room right.
    fn menu_box(&mut self, items: &[Option<Item>], at: (i32, i32), beside: Option<Rect>) -> MenuBox {
        let ui = &mut self.fonts.ui;
        let pad = (self.scale * 8.0).round() as i32;
        let row_h = ui.height + pad;
        let sep_h = pad;
        let widest = items.iter().flatten().map(|i| ui.width(&i.label) + if i.hint.is_empty() { 0 } else { ui.width(&i.hint) + 3 * pad }).max().unwrap_or(0);
        let w = widest + 4 * pad;
        let h = items.iter().map(|i| if i.is_some() { row_h } else { sep_h }).sum::<i32>() + pad;
        let (mut x, mut y) = at;
        match beside {
            Some(p) => {
                x = p.x + p.w - pad / 4;
                if x + w > self.canvas.width {
                    x = (p.x - w + pad / 4).max(0);
                }
                y = (y - pad / 2).min(self.canvas.height - h).max(0);
            }
            None => {
                if x + w > self.canvas.width {
                    x = (x - w).max(0);
                }
                if y + h > self.canvas.height {
                    y = (y - h).max(0);
                }
            }
        }
        MenuBox { rect: Rect { x, y, w, h }, pad, row_h, sep_h }
    }

    /// The menu's box, and the open submenu's with its items.
    pub(super) fn menu_boxes(&mut self) -> Option<(MenuBox, Option<(MenuBox, Vec<Option<Item>>)>)> {
        let m = self.menu.as_ref()?;
        let (items, at, open) = (m.items.clone(), m.at, m.open);
        let sub = open.and_then(|(row, _)| match items.get(row)?.as_ref()?.action {
            Action::Submenu(k) => Some((row, m.children.get(k)?.clone())),
            _ => None,
        });
        let main = self.menu_box(&items, at, None);
        let sub = sub.map(|(row, kids)| {
            let top = main.tops(&items)[row];
            (self.menu_box(&kids, (0, top), Some(main.rect)), kids)
        });
        Some((main, sub))
    }

    fn draw_menu_box(&mut self, b: MenuBox, items: &[Option<Item>], hover: Option<usize>, whole: bool) {
        let c = self.cur_theme.chrome();
        let MenuBox { rect: r, pad, row_h, sep_h } = b;
        if whole {
            self.canvas.panel(r, pad, c.panel, c.edge);
        } else {
            // Only its rows again (a row lit, another not): the inside
            // filled afresh, its edge and shadow as they were, drawing
            // them again would darken the shadow.
            self.canvas.fill(Rect { x: r.x + pad / 2, y: r.y + pad / 2, w: r.w - pad, h: r.h - pad }, c.panel);
        }
        let ui = &mut self.fonts.ui;
        for (i, top) in b.tops(items).into_iter().enumerate() {
            let Some(item) = &items[i] else {
                self.canvas.fill(Rect { x: r.x + pad, y: top + sep_h / 2, w: r.w - 2 * pad, h: 1 }, c.edge);
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
        }
    }

    /// The menu, and its open submenu: drawn `whole` in a full frame, or
    /// only their rows when the row lit is all that changed, which is all
    /// that goes to the screen then (not the window under it, again).
    pub(super) fn draw_menu(&mut self, whole: bool) {
        let Some((main, sub)) = self.menu_boxes() else { return };
        let Some(m) = self.menu.as_ref() else { return };
        let (items, open) = (m.items.clone(), m.open);
        // The row a submenu hangs from stays lit while it is open.
        let hover = open.map(|(row, _)| row).or(m.hover);
        // What was under the submenu put back (it may be gone, or another
        // open), unless the frame is drawn whole anyway.
        let under = self.menu_under.take();
        if let (false, Some(u)) = (whole, &under) {
            self.canvas.restore(u);
            self.damage.push(u.0);
        }
        self.draw_menu_box(main, &items, hover, whole);
        if !whole {
            self.damage.push(main.rect);
        }
        // The submenu drawn whole, its shadow too, over what is kept to
        // put back when it goes: opening or closing one is its own patch,
        // not the window drawn again.
        if let Some((b, kids)) = sub {
            let s = (self.scale * 24.0).round().max(24.0) as i32;
            let area = Rect { x: b.rect.x - s, y: b.rect.y - s, w: b.rect.w + 2 * s, h: b.rect.h + 2 * s };
            self.menu_under = Some(self.canvas.save(area));
            self.draw_menu_box(b, &kids, open.and_then(|(_, h)| h), true);
            if !whole {
                self.damage.push(area);
            }
        }
        if whole {
            self.damage.push(Rect { x: 0, y: 0, w: self.canvas.width, h: self.canvas.height });
        }
    }

    /// What is under the pointer in the menu: a row of the submenu, or
    /// of the menu itself, as (in the submenu, row).
    fn menu_hit(&mut self, x: i32, y: i32) -> Option<(bool, Option<usize>)> {
        let (main, sub) = self.menu_boxes()?;
        if let Some((b, kids)) = &sub {
            if b.rect.contains(x, y) {
                return Some((true, b.row_at(kids, x, y)));
            }
        }
        let items = self.menu.as_ref()?.items.clone();
        main.rect.contains(x, y).then(|| (false, main.row_at(&items, x, y)))
    }

    /// A click while something floats: the palette or the menu gets it.
    /// True when it was theirs.
    pub(super) fn overlay_click(&mut self, button: MouseButton, event_loop: &ActiveEventLoop) -> bool {
        let (x, y) = self.pointer;
        if self.menu.is_some() {
            let hit = self.menu_hit(x, y);
            let item = match hit {
                Some((true, Some(row))) => self.menu.as_ref().and_then(|m| {
                    let (parent, _) = m.open?;
                    let Action::Submenu(k) = m.items[parent].as_ref()?.action else { return None };
                    m.children.get(k)?.get(row)?.clone()
                }),
                Some((false, Some(row))) => self.menu.as_ref().and_then(|m| m.items[row].clone()),
                _ => None,
            };
            match item {
                // A row with a submenu opens it, as hovering does.
                Some(Item { action: Action::Submenu(_), .. }) => {
                    if let (Some(m), Some((false, Some(row)))) = (&mut self.menu, hit) {
                        m.open = Some((row, None));
                    }
                    self.full = true;
                    self.redraw();
                }
                Some(item) => {
                    self.close_overlay();
                    if button == MouseButton::Left {
                        self.run(item.action, event_loop);
                    }
                }
                // On a line between groups: nothing. Elsewhere: gone.
                None if hit.is_some() => {}
                None => self.close_overlay(),
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
            let hit = self.menu_hit(x, y);
            let Some(m) = &mut self.menu else { return true };
            let (hover, open) = match hit {
                Some((true, h)) => (None, m.open.map(|(p, _)| (p, h))),
                Some((false, Some(row))) => match m.items[row].as_ref().map(|i| &i.action) {
                    Some(Action::Submenu(_)) => (Some(row), Some((row, m.open.filter(|(p, _)| *p == row).and_then(|(_, h)| h)))),
                    _ => (Some(row), None),
                },
                // Off any row: what is open stays open, so the pointer can
                // cross to it.
                _ => (None, m.open.map(|(p, _)| (p, None))),
            };
            if m.hover != hover || m.open != open {
                // Only the menu drawn again: its rows, and a submenu come or
                // gone as a patch of its own (see draw_menu).
                self.menu_relit = true;
                m.hover = hover;
                m.open = open;
                self.redraw();
            }
            return true;
        }
        if self.palette.is_some() {
            if let Some(row) = self.palette_row_at(x, y) {
                if let Some(p) = &mut self.palette {
                    if p.selected != row {
                        p.selected = row;
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
            let m = &self.machines[self.focused().map_or(0, |f| crate::mux::split_id(f).0).min(self.machines.len() - 1)];
            match &m.host {
                Some(host) if !m.client.connected() => w.set_title(&format!("{} · {host}, reconnecting… · jot", ws.name)),
                Some(host) => w.set_title(&format!("{} · {host} · jot", ws.name)),
                None => w.set_title(&format!("{} · jot", ws.name)),
            }
        }
    }
}

impl App {
    /// The menu for a tag on a pane's caption, at the pointer.
    pub(super) fn open_tag_menu(&mut self, id: PaneId, kind: super::watch::Tag) {
        use super::watch::Tag;
        let c = |label: &str, hint: &str, action| Some(Item { label: label.into(), hint: hint.into(), action });
        let mut items: Vec<Option<Item>> = match kind {
            Tag::Multi => {
                let skipped = self.typed_into(id) == Some(false);
                let any_skipped = self.workspaces.get(self.current).is_some_and(|w| !w.left_out.is_empty());
                vec![
                    c("Stop multi", "Alt Shift M", Action::Broadcast),
                    None,
                    c(if skipped { "Include this pane" } else { "Skip this pane" }, "click", Action::MultiSkip(id)),
                    if any_skipped { c("Include all panes", "", Action::MultiAll) } else { None },
                ]
            }
            Tag::Tee => {
                let name = self.panes.get(&id).and_then(|p| p.tee.as_ref()).and_then(|t| t.path.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                vec![c(&format!("Open {name}"), "", Action::TeeOpen(id)), c("Copy path", "", Action::TeeCopyPath(id)), None, c("Stop", "", Action::TeeStop(id))]
            }
            Tag::Filter => vec![c("Stop", "click", Action::FilterStop(id))],
            Tag::Notify => vec![c("Cancel", "click", Action::NotifyOff(id))],
            Tag::TeeStop => vec![c("Stop", "", Action::TeeStop(id))],
            Tag::Focus => vec![c(self.state_label(&Action::Zoom).unwrap_or_default(), "Alt Shift F", Action::Zoom)],
            Tag::Restart => vec![c("Restart", "Ctrl R", Action::Restart)],
            Tag::Minimize => vec![c("Minimize", "", Action::Minimize)],
            Tag::Close => vec![c("Close pane (it runs on)", "Alt X", Action::Close), c("Kill terminal", "", Action::EndPane)],
        };
        while matches!(items.last(), Some(None)) {
            items.pop();
        }
        self.palette = None;

        self.menu_at = self.pointer;
        self.menu = Some(Menu { items, at: self.pointer, hover: None, children: Vec::new(), open: None });
        self.full = true;
        self.redraw();
    }
}

impl App {
    /// What a command that turns something on and off is called now: by
    /// what it would do.
    pub(super) fn state_label(&self, action: &Action) -> Option<&'static str> {
        Some(match action {
            Action::Broadcast if self.ws().broadcast => "Stop multi",
            Action::Broadcast => "Start multi",
            Action::Zoom if self.ws().zoom.is_some() => "Restore",
            Action::Zoom => "Maximize",
            Action::StatusLine if self.config.status_line => "Hide status line",
            Action::StatusLine => "Show status line",
            _ => return None,
        })
    }
}

impl App {
    /// A menu of these items at the pointer: no line at its ends.
    pub(super) fn open_items_menu(&mut self, mut items: Vec<Option<Item>>) {
        // Items left out leave their separators; one between groups is enough.
        items.dedup_by(|a, b| a.is_none() && b.is_none());
        while matches!(items.last(), Some(None)) {
            items.pop();
        }
        while matches!(items.first(), Some(None)) {
            items.remove(0);
        }
        self.palette = None;
        self.menu_at = self.pointer;
        self.menu = Some(Menu { items, at: self.pointer, hover: None, children: Vec::new(), open: None });
        self.full = true;
        self.redraw();
    }
}

/// The machines to offer: the hosts named in ~/.ssh/config (no wildcards),
/// those with `typed` in them; and what is typed, as ssh would take it.
fn machine_items(typed: &str, add: bool) -> Vec<Item> {
    let act = |h: String| if add { Action::AddMachine(h) } else { Action::Connect(h) };
    let config = std::fs::read_to_string(crate::pty::home().join(".ssh/config")).unwrap_or_default();
    let mut hosts: Vec<String> = Vec::new();
    for line in config.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("Host ").or_else(|| line.strip_prefix("host ")) else { continue };
        for h in rest.split_whitespace() {
            if !h.contains(['*', '?', '!']) && !hosts.iter().any(|x| x == h) {
                hosts.push(h.to_string());
            }
        }
    }
    let t = typed.trim();
    let mut items: Vec<Item> = hosts.into_iter().filter(|h| h.to_lowercase().contains(&t.to_lowercase())).map(|h| Item { label: h.clone(), hint: "ssh config".into(), action: act(h) }).collect();
    if !t.is_empty() && !items.iter().any(|i| i.label == t) {
        items.push(Item { label: t.to_string(), hint: "connect".into(), action: act(t.to_string()) });
    }
    items
}

impl App {
    /// The box for a machine to add to the sidebar.
    pub(super) fn add_machine_prompt(&mut self) {
        self.palette = Some(Palette { mode: Mode::AddMachine, field: TextField::default(), matches: Vec::new(), selected: 0, first: 0 });
        self.palette_changed();
    }
}
