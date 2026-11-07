//! Slash commands, typed at a shell prompt: a `/` as the first thing on
//! the line opens a list by the cursor that narrows as you type. The keys
//! still go to the shell as they are typed; Tab or Enter takes the pick,
//! rubbing out the `/…` typed and doing the command. A second `/`, or
//! nothing left that matches, and the list goes, the line being a path.

use super::settings::Hit;
use super::*;
use crate::canvas::on;
use crate::palette::Action;
use crate::theme::THEMES;

/// What a slash command does.
#[derive(Clone, Debug)]
pub enum Does {
    /// One of jterm's own, as the palette and the menu run them.
    Act(Action),
    /// A theme, chosen from a list after a space.
    Theme,
    /// Text put on the command line, not run.
    Insert(String),
    /// A command line typed and run.
    Run(String),
    /// The pane's running job: Ctrl C to it, or killed.
    Stop,
    /// What is running in the pane, and the pane itself.
    Info,
    /// The command line in the long-command editor.
    Edit,
    /// Output to a file too; only lines with some text shown; matches
    /// found and lit, as Ctrl F does. Each takes what follows it.
    Tee,
    Filter,
    Grep,
    /// The workspace given the name that follows.
    Rename,
    /// Told when the command ends; its processes; its ports; its files.
    Notify,
    Tree,
    Ports,
    Files,
    Kill,
}

#[derive(Clone, Debug)]
pub struct Command {
    pub name: String,
    pub what: String,
    pub does: Does,
}

impl Command {
    fn new(name: &str, what: &str, does: Does) -> Command {
        Command { name: name.into(), what: what.into(), does }
    }

    /// Whether it takes a word after it, picked from a list.
    fn takes_arg(&self) -> bool {
        matches!(self.does, Does::Theme | Does::Tee | Does::Filter | Does::Grep | Does::Rename)
    }
}

/// A row of the list: a command, or a value for one.
#[derive(Clone, Debug)]
pub struct Row {
    pub label: String,
    pub what: String,
    /// The group it is from, shown at the right.
    pub kind: &'static str,
}

/// The list open for a pane: what has been typed after the `/`.
pub struct Slash {
    pub pane: PaneId,
    pub typed: String,
    pub selected: usize,
    pub first: usize,
    /// Opened after a space mid-line, not at the start: only what puts
    /// text on the line, by the start of its name.
    pub mid: bool,
}

const SHOWN: usize = 8;

impl App {
    /// The commands there are: jterm's, then the config's own.
    pub(super) fn slash_commands(&self) -> Vec<Command> {
        let mut all = vec![
            Command::new("settings", "Open settings", Does::Act(Action::Settings)),
            Command::new("info", "What this pane is running: pid, time, memory", Does::Info),
            Command::new("notify", "Tell me when this command ends", Does::Notify),
            Command::new("tree", "The running command's processes", Does::Tree),
            Command::new("ports", "Ports the running command listens on", Does::Ports),
            Command::new("files", "Files the running command has open", Does::Files),
            Command::new("split", "Split right", Does::Act(Action::Split(Dir::Across))),
            Command::new("split-down", "Split down", Does::Act(Action::Split(Dir::Down))),
            if self.ws().zoom.is_some() {
                Command::new("unfocus", "Unfocus pane: show all", Does::Act(Action::Zoom))
            } else {
                Command::new("focus", "Focus pane: this one alone", Does::Act(Action::Zoom))
            },
            if self.ws().broadcast {
                Command::new("multi", "Stop multi", Does::Act(Action::Broadcast))
            } else {
                Command::new("multi", "Start multi: type into every pane", Does::Act(Action::Broadcast))
            },
            Command::new("new-workspace", "New workspace in this folder", Does::Act(Action::NewWorkspace)),
            Command::new("rename", "Rename this workspace", Does::Rename),
            Command::new("clear", "Clear terminal", Does::Act(Action::Clear)),
            Command::new("theme", "Set the color theme", Does::Theme),
            Command::new("edit", "Edit this command in a larger editor", Does::Edit),
            Command::new("tee", "Write this pane's output to a file as well", Does::Tee),
            Command::new("filter", "Show only the running command's lines with some text", Does::Filter),
            Command::new("grep", "Find and light matches, as Ctrl F", Does::Grep),
        ];
        // For a pane's own command line: its running job stopped or killed.
        if self.bar.is_some() {
            all.insert(0, Command::new("stop", "Ctrl C to the running job", Does::Stop));
            all.insert(1, Command::new("kill", "Kill the running job (again: kill -9)", Does::Kill));
        }
        // The config's: "name" = "text" puts the text on the line; a name
        // starting with ! runs it.
        for (name, text) in &self.config.slash {
            let (name, does) = match name.strip_prefix('!') {
                Some(n) => (n, Does::Run(text.clone())),
                None => (name.as_str(), Does::Insert(text.clone())),
            };
            all.push(Command::new(name, text, does));
        }
        all
    }

    /// The command named exactly as typed before a space, if it takes a
    /// word after it.
    fn slash_arg_command(&self, typed: &str) -> Option<(Command, String)> {
        let (name, arg) = typed.split_once(' ')?;
        let c = self.slash_commands().into_iter().find(|c| c.name == name && c.takes_arg())?;
        Some((c, arg.to_string()))
    }

    /// The values a command takes, with what each is.
    fn slash_values(&self, does: &Does) -> Vec<(String, String)> {
        match does {
            Does::Theme => THEMES.iter().map(|t| (t.name.to_lowercase().replace(' ', "-"), String::new())).collect(),
            _ => Vec::new(),
        }
    }

    /// The rows for what has been typed: commands whose names start with
    /// it, then those that have it anywhere; or, after a command that
    /// takes a word and a space, its values the same way.
    pub(super) fn slash_rows(&self, typed: &str, mid: bool) -> Vec<Row> {
        let rank = |name: &str, q: &str| -> Option<u8> {
            let (n, q) = (name.to_lowercase(), q.to_lowercase());
            if n.starts_with(&q) {
                Some(0)
            } else if n.contains(&q) {
                Some(1)
            } else {
                None
            }
        };
        if mid && typed.is_empty() {
            return Vec::new();
        }
        if let Some((c, arg)) = self.slash_arg_command(typed).filter(|_| !mid) {
            // Free text after it: one row, saying what it will do.
            if matches!(c.does, Does::Tee | Does::Filter | Does::Grep | Does::Rename) {
                let arg = arg.trim();
                let what = match (&c.does, arg.is_empty()) {
                    (Does::Tee, true) if self.focused().and_then(|id| self.panes.get(&id)).is_some_and(|p| p.tee.is_some()) => "Stop".to_string(),
                    (Does::Tee, true) => "Save the next command's output to a new file".to_string(),
                    (Does::Tee, false) => "Save the command's output here too (--all: everything)".to_string(),
                    (Does::Filter, true) => "Stop filtering".to_string(),
                    (Does::Filter, false) => "Show only lines with this".to_string(),
                    (Does::Rename, true) => "Type the new name, or Enter for the rename box".to_string(),
                    (Does::Rename, false) => "The new name for this workspace".to_string(),
                    (_, true) => "Type what to find".to_string(),
                    _ => "Find and light it".to_string(),
                };
                let label = if arg.is_empty() { c.name.clone() } else { format!("{} {arg}", c.name) };
                return vec![Row { label, what, kind: kind_of(&c.does) }];
            }
            let mut rows: Vec<(u8, Row)> = self
                .slash_values(&c.does)
                .into_iter()
                .filter_map(|(v, what)| rank(&v, &arg).map(|r| (r, Row { label: format!("{} {v}", c.name), what, kind: kind_of(&c.does) })))
                .collect();
            rows.sort_by_key(|(r, _)| *r);
            return rows.into_iter().map(|(_, r)| r).collect();
        }
        if typed.contains(' ') {
            return Vec::new();
        }
        let mut rows: Vec<(u8, Row)> = self
            .slash_commands()
            .into_iter()
            .filter_map(|c| {
                // Mid-line, /tee takes no word: it is the line's own command.
                let label = if c.takes_arg() && !mid { format!("{} …", c.name) } else { c.name.clone() };
                if mid && !(matches!(c.does, Does::Insert(_) | Does::Edit | Does::Tee) && c.name.starts_with(typed)) {
                    return None;
                }
                rank(&c.name, typed).map(|r| (r, Row { label, what: c.what.clone(), kind: kind_of(&c.does) }))
            })
            .collect();
        rows.sort_by_key(|(r, _)| *r);
        rows.into_iter().map(|(_, r)| r).collect()
    }

    /// A key while typing at a prompt. Opens the list on a `/` that
    /// starts the line; with it open, ↑ ↓ pick, Tab and Enter take, Esc
    /// closes, and what is typed narrows it. True when the key was the
    /// list's and is not to go to the shell.
    pub(super) fn slash_key(&mut self, id: PaneId, code: KeyCode, text: &str, event_loop: &ActiveEventLoop) -> bool {
        let plain = !self.mods.control_key() && !self.mods.alt_key() && !self.mods.super_key();
        let before = self.panes.get(&id).filter(|p| p.shell.at_prompt).and_then(|p| p.shell.line.clone());
        self.follow_line(id, code, text, plain);
        // A key the list takes leaves the line as it was.
        let restore = |app: &mut App| {
            if let Some(p) = app.panes.get_mut(&id) {
                p.shell.line = before.clone();
            }
        };
        let Some(s) = &self.slash else {
            if plain {
                self.slash_open(id, code, text, before.as_deref() == Some(""));
            }
            return false;
        };
        if s.pane != id {
            self.close_slash();
            return false;
        }
        let n = self.slash_rows(&s.typed, s.mid).len();
        match code {
            KeyCode::ArrowDown | KeyCode::ArrowUp if plain => {
                restore(self);
                if let Some(s) = &mut self.slash {
                    s.selected = if code == KeyCode::ArrowDown { (s.selected + 1).min(n.saturating_sub(1)) } else { s.selected.saturating_sub(1) };
                    s.first = s.first.min(s.selected).max((s.selected + 1).saturating_sub(SHOWN));
                }
                self.full = true;
                self.redraw();
                true
            }
            KeyCode::Escape => {
                restore(self);
                self.close_slash();
                true
            }
            KeyCode::Tab | KeyCode::Enter | KeyCode::NumpadEnter if plain && n > 0 => {
                restore(self);
                self.slash_take(event_loop, code != KeyCode::Tab);
                true
            }
            KeyCode::Backspace => {
                let gone = self.slash.as_mut().is_some_and(|s| s.typed.pop().is_none());
                if gone {
                    self.close_slash();
                } else {
                    self.slash_narrowed();
                }
                false
            }
            _ if plain && !text.is_empty() && !text.chars().any(char::is_control) => {
                // A second slash, or a space mid-line: a path, or words,
                // after all.
                let mid = self.slash.as_ref().is_some_and(|s| s.mid);
                let has_word = self.slash.as_ref().is_some_and(|s| s.typed.contains(' '));
                if (text.contains('/') && !has_word) || (mid && text.contains(char::is_whitespace)) {
                    self.close_slash();
                    return false;
                }
                if let Some(s) = &mut self.slash {
                    s.typed.push_str(text);
                }
                self.slash_narrowed();
                false
            }
            // Anything else, Ctrl C, the arrows across: the list goes and
            // the key is the shell's.
            _ => {
                self.close_slash();
                false
            }
        }
    }

    /// The list opened, when the line now calls for it: a `/` typed on an
    /// empty line; a space, a `/` and a first letter mid-line that start
    /// the name of something that puts text on the line; or the line
    /// rubbed back to either.
    fn slash_open(&mut self, id: PaneId, code: KeyCode, text: &str, was_empty: bool) {
        if text == "/" && was_empty {
            self.slash = Some(Slash { pane: id, typed: String::new(), selected: 0, first: 0, mid: false });
            self.full = true;
            return self.redraw();
        }
        let typing = !text.is_empty() && !text.contains('/') && !text.chars().any(|c| c.is_control() || c.is_whitespace());
        if code != KeyCode::Backspace && !typing {
            return;
        }
        let Some(line) = self.panes.get(&id).filter(|p| p.shell.at_prompt).and_then(|p| p.shell.line.clone()) else { return };
        let Some(at) = line.rfind('/') else { return };
        let (head, typed) = (&line[..at], &line[at + 1..]);
        if typed.contains(char::is_whitespace) {
            return;
        }
        let mid = !head.is_empty();
        // Mid-line, only right after a space, and while typing only on the
        // first letter: words further on are left alone.
        if mid && (!head.ends_with(char::is_whitespace) || typed.is_empty() || (code != KeyCode::Backspace && typed.chars().count() != 1)) {
            return;
        }
        if !mid && code != KeyCode::Backspace {
            return;
        }
        if !self.slash_rows(typed, mid).is_empty() {
            self.slash = Some(Slash { pane: id, typed: typed.to_string(), selected: 0, first: 0, mid });
            self.full = true;
            self.redraw();
        }
    }

    /// What is on the command line, followed key by key: printing keys
    /// add, Backspace takes away, Ctrl U and Ctrl C empty it; any other
    /// key makes it unknown until the next prompt.
    fn follow_line(&mut self, id: PaneId, code: KeyCode, text: &str, plain: bool) {
        let ctrl = self.mods.control_key();
        let Some(p) = self.panes.get_mut(&id) else { return };
        let line = &mut p.shell.line;
        if matches!(code, KeyCode::Enter | KeyCode::NumpadEnter) {
            p.shell.entered = line.clone();
        }
        let next = match code {
            KeyCode::KeyU | KeyCode::KeyC if ctrl => Some(String::new()),
            KeyCode::Backspace if plain => line.take().map(|mut l| {
                l.pop();
                l
            }),
            _ if plain && !text.is_empty() && !text.chars().any(char::is_control) => line.take().map(|l| l + text),
            KeyCode::ShiftLeft | KeyCode::ShiftRight | KeyCode::ControlLeft | KeyCode::ControlRight | KeyCode::AltLeft | KeyCode::AltRight => line.take(),
            _ => None,
        };
        *line = next;
    }

    /// After what is typed changed: back to the first row, or the list
    /// gone when nothing matches.
    fn slash_narrowed(&mut self) {
        let (typed, mid) = self.slash.as_ref().map(|s| (s.typed.clone(), s.mid)).unwrap_or_default();
        if self.slash_rows(&typed, mid).is_empty() && (mid || self.slash_arg_command(&typed).is_none()) {
            return self.close_slash();
        }
        if let Some(s) = &mut self.slash {
            s.selected = 0;
            s.first = 0;
        }
        self.full = true;
        self.redraw();
    }

    pub(super) fn close_slash(&mut self) {
        if self.slash.take().is_some() {
            self.full = true;
            self.redraw();
        }
    }

    /// The pick taken. A command that wants a word, taken with only its
    /// name typed, is filled in on the line for the word to be picked;
    /// anything else rubs out the `/…` and is done.
    fn slash_take(&mut self, event_loop: &ActiveEventLoop, enter: bool) {
        let Some(s) = &self.slash else { return };
        let (id, typed, selected, mid) = (s.pane, s.typed.clone(), s.selected, s.mid);
        let rows = self.slash_rows(&typed, mid);
        let Some(row) = rows.get(selected) else { return };
        let write = |app: &mut App, bytes: &[u8]| {
            for t in app.input_targets(id) {
                if let Some(p) = app.panes.get(&t) {
                    p.pty.write(bytes);
                }
            }
        };
        // A command that takes a word: its name and a space on the line,
        // and the list of its words.
        if let Some(name) = row.label.strip_suffix(" …") {
            let rest = format!("{name} ");
            let add = rest.strip_prefix(typed.as_str()).map(str::to_string).unwrap_or_else(|| {
                write(self, &vec![0x7f; typed.chars().count()]);
                rest.clone()
            });
            write(self, add.as_bytes());
            if let Some(p) = self.panes.get_mut(&id) {
                p.shell.line = Some(format!("/{rest}"));
            }
            if let Some(s) = &mut self.slash {
                s.typed = rest;
                s.selected = 0;
                s.first = 0;
            }
            self.full = true;
            return self.redraw();
        }
        let label = row.label.clone();
        // The `/` and what follows it, rubbed out from the line.
        if let Some(p) = self.panes.get_mut(&id) {
            let cut = format!("/{typed}");
            p.shell.line = p.shell.line.take().and_then(|l| l.strip_suffix(cut.as_str()).map(str::to_string));
        }
        write(self, &vec![0x7f; typed.chars().count() + 1]);
        self.slash = None;
        self.full = true;
        let (name, arg) = label.split_once(' ').map(|(n, a)| (n.to_string(), a.to_string())).unwrap_or((label.clone(), String::new()));
        let Some(c) = self.slash_commands().into_iter().find(|c| c.name == name) else { return };
        // /tee at the end of a line, taken with Enter: the line run, teed.
        let run = mid && enter && matches!(c.does, Does::Tee);
        self.slash_apply(id, c.does, &arg, event_loop);
        if run {
            write(self, b"\r");
            if let Some(p) = self.panes.get_mut(&id) {
                p.shell.entered = p.shell.line.take();
            }
        }
    }

    /// A slash command done, for pane `id`; `arg` is the word after it.
    pub(super) fn slash_apply(&mut self, id: PaneId, does: Does, arg: &str, event_loop: &ActiveEventLoop) {
        let write = |app: &mut App, bytes: &[u8]| {
            for t in app.input_targets(id) {
                if let Some(p) = app.panes.get(&t) {
                    p.pty.write(bytes);
                }
            }
        };
        match does {
            Does::Act(a) => self.run(a, event_loop),
            Does::Theme => {
                if let Some(i) = THEMES.iter().position(|t| t.name.to_lowercase().replace(' ', "-") == *arg) {
                    self.settings_do(Hit::Theme(i));
                }
            }
            Does::Insert(text) => self.slash_insert(id, &text),
            Does::Run(cmd) => write(self, format!("{cmd}\r").as_bytes()),
            Does::Stop => self.stop_job(id),
            Does::Info => self.print_info(id),
            Does::Notify => self.set_notify(id, true),
            Does::Tree => {
                let lines = self.report_tree(id);
                self.print_report(id, "/tree", lines);
            }
            Does::Ports => {
                let lines = self.report_ports(id);
                self.print_report(id, "/ports", lines);
            }
            Does::Files => {
                let lines = self.report_files(id);
                self.print_report(id, "/files", lines);
            }
            Does::Tee => self.tee(id, arg),
            Does::Filter => self.filter(id, arg),
            Does::Grep => self.find_text(id, arg),
            Does::Rename => match arg.trim() {
                "" => self.run(Action::Rename, event_loop),
                name => {
                    if let Some(ws) = self.workspaces.iter_mut().find(|w| w.root.contains(id)) {
                        ws.name = name.to_string();
                    }
                    self.unsaved = true;
                    self.set_title();
                    self.full = true;
                }
            },
            Does::Edit => {
                let text = self.prompt_line(id);
                self.open_editor_with(id, &text, false);
            }
            Does::Kill => self.kill_job(id),
        }
        self.redraw();
    }

    /// Text put on the command line, counted as typed.
    fn slash_insert(&mut self, id: PaneId, text: &str) {
        for t in self.input_targets(id) {
            if let Some(p) = self.panes.get(&t) {
                p.pty.write(text.as_bytes());
            }
        }
        if let Some(p) = self.panes.get_mut(&id) {
            if let Some(l) = &mut p.shell.line {
                l.push_str(text);
            }
        }
    }

    /// The list, by the cursor: under it, or over it near the bottom.
    pub(super) fn draw_slash(&mut self) {
        let Some(s) = &self.slash else { return };
        let (id, typed, selected, first, mid) = (s.pane, s.typed.clone(), s.selected, s.first, s.mid);
        let Some(p) = self.panes.get(&id) else { return };
        if !p.rect.contains(p.content.x, p.content.y) {
            return;
        }
        let cur = p.term.cursor();
        let (cw, ch) = (self.fonts.cell_w, self.fonts.cell_h);
        let content = p.content;
        let rows = self.slash_rows(&typed, mid);
        // Its left at the slash, one column back from the cursor.
        let x = content.x + (cur.x as i32 - 1 - typed.chars().count() as i32).max(0) * cw;
        let below = content.y + (cur.y as i32 + 1) * ch + self.px(4.0);
        let above = content.y + cur.y as i32 * ch - self.px(4.0);
        self.draw_command_list(&rows, selected, first, x, below, above, false);
    }

    /// A list of slash commands at `x`: its top at `below`, or its bottom
    /// at `above` when it is asked to rise or would not fit under.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_command_list(&mut self, rows: &[Row], selected: usize, first: usize, x: i32, below: i32, above: i32, rise: bool) {
        let c = self.cur_theme.chrome();
        let accent = self.cur_accent;
        let scale = self.scale;
        let px = |v: f32| (v * scale).round() as i32;
        let (uh, sh) = (self.fonts.ui.height, self.fonts.small.height);
        let row_h = uh + px(10.0);
        let shown: Vec<(usize, Row)> = rows.iter().cloned().enumerate().skip(first).take(SHOWN).collect();
        if shown.is_empty() {
            return;
        }
        let name_w = shown.iter().map(|(_, r)| self.fonts.ui.width(&format!("/{}", r.label))).max().unwrap_or(0);
        let what_w = shown.iter().map(|(_, r)| self.fonts.small.width(&r.what)).max().unwrap_or(0);
        let kind_w = shown.iter().map(|(_, r)| self.fonts.small.width(r.kind)).max().unwrap_or(0);
        let w = (px(12.0) + name_w + px(16.0) + what_w + px(16.0) + kind_w + px(12.0)).min(self.canvas.width - px(16.0));
        let h = shown.len() as i32 * row_h + px(8.0);
        let x = x.min(self.canvas.width - w - px(8.0)).max(px(8.0));
        let fits_below = below + h <= self.canvas.height - px(8.0);
        let y = if !rise && fits_below { below } else { (above - h).max(px(8.0)) };
        let r = Rect { x, y, w, h };
        self.canvas.panel(r, px(8.0), c.panel, c.edge);
        for (k, (i, row)) in shown.iter().enumerate() {
            let top = r.y + px(4.0) + k as i32 * row_h;
            let picked = *i == selected;
            if picked {
                self.canvas.round_fill(Rect { x: r.x + px(4.0), y: top, w: r.w - px(8.0), h: row_h }, px(6.0), accent, 255);
            }
            let (main, dim) = if picked { (on(accent), mix(accent, on(accent), 190)) } else { (c.text, c.dim) };
            let ty = top + (row_h - uh) / 2;
            let nx = r.x + px(12.0);
            self.canvas.text(&mut self.fonts.ui, &format!("/{}", row.label), nx as f32, ty, main, r);
            let wx = nx + name_w + px(16.0);
            self.canvas.text_fit(&mut self.fonts.small, &row.what, wx, top + (row_h - sh) / 2, (r.x + r.w - px(28.0) - kind_w - wx).max(0), dim, r);
            let kw = self.fonts.small.width(row.kind);
            self.canvas.text(&mut self.fonts.small, row.kind, (r.x + r.w - px(12.0) - kw) as f32, top + (row_h - sh) / 2, if picked { dim } else { c.faint }, r);
        }
        self.damage.push(Rect { x: r.x - px(8.0), y: r.y - px(8.0), w: r.w + px(16.0), h: r.h + px(16.0) });
    }
}

fn kind_of(d: &Does) -> &'static str {
    match d {
        Does::Act(_) => "jterm",
        Does::Theme => "setting",
        Does::Insert(_) => "insert",
        Does::Edit => "edit",
        Does::Tee | Does::Filter => "watch",
        Does::Grep => "find",
        Does::Rename => "jterm",
        Does::Run(_) => "run",
        Does::Stop | Does::Kill | Does::Info | Does::Notify | Does::Tree | Does::Ports | Does::Files => "job",
    }
}
