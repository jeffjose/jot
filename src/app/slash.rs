//! Slash commands, typed at a shell prompt: a `/` as the first thing on
//! the line opens a list by the cursor that narrows as you type. The keys
//! still go to the shell as they are typed; Tab or Enter takes the pick,
//! rubbing out the `/…` typed and doing the command. A second `/`, or
//! nothing left that matches, and the list goes, the line being a path.

use super::settings::{Hit, Key};
use super::*;
use crate::canvas::on;
use crate::palette::{self, Action};
use crate::theme::{ACCENTS, THEMES};

/// What a slash command does.
#[derive(Clone, Debug)]
pub enum Does {
    /// One of jterm's own, as the palette and the menu run them.
    Act(Action),
    /// A setting switched on or off.
    Toggle(Key),
    /// A theme, an accent, a project folder: chosen from a list after a
    /// space.
    Theme,
    Accent,
    Cd,
    /// Text put on the command line, not run.
    Insert(String),
    /// A command line typed and run.
    Run(String),
    /// The date, or the folder the shell is in, put on the line.
    Date,
    Cwd,
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
        matches!(self.does, Does::Theme | Does::Accent | Does::Cd)
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
}

const SHOWN: usize = 8;

impl App {
    /// The commands there are: jterm's, then the config's own.
    fn slash_commands(&self) -> Vec<Command> {
        let mut all = vec![
            Command::new("settings", "Open settings", Does::Act(Action::Settings)),
            Command::new("split", "Split right", Does::Act(Action::Split(Dir::Across))),
            Command::new("split-down", "Split down", Does::Act(Action::Split(Dir::Down))),
            Command::new("focus", "Focus pane / show all", Does::Act(Action::Zoom)),
            Command::new("multi", "Type into multiple panes", Does::Act(Action::Broadcast)),
            Command::new("new-workspace", "New workspace in this folder", Does::Act(Action::NewWorkspace)),
            Command::new("rename", "Rename workspace", Does::Act(Action::Rename)),
            Command::new("clear", "Clear terminal", Does::Act(Action::Clear)),
            Command::new("theme", "Set the color theme", Does::Theme),
            Command::new("accent", "Set the accent color", Does::Accent),
            Command::new("ligatures", "Turn ligatures on or off", Does::Toggle(Key::Ligatures)),
            Command::new("bold", "Turn bold text on or off", Does::Toggle(Key::Bold)),
            Command::new("captions", "Turn pane captions on or off", Does::Toggle(Key::Captions)),
            Command::new("cd", "Go to a project folder", Does::Cd),
            Command::new("date", "Insert today's date", Does::Date),
            Command::new("cwd", "Insert this folder's path", Does::Cwd),
        ];
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
            Does::Accent => ACCENTS.iter().map(|(n, _)| (n.to_string(), String::new())).collect(),
            Does::Cd => {
                let roots: Vec<PathBuf> = self.config.projects.iter().map(|p| persist::expand(p)).collect();
                palette::projects(&roots).into_iter().map(|d| (folder_name(&d), persist::shorten(&d))).collect()
            }
            _ => Vec::new(),
        }
    }

    /// The rows for what has been typed: commands whose names start with
    /// it, then those that have it anywhere; or, after a command that
    /// takes a word and a space, its values the same way.
    pub(super) fn slash_rows(&self, typed: &str) -> Vec<Row> {
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
        if let Some((c, arg)) = self.slash_arg_command(typed) {
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
                let label = if c.takes_arg() { format!("{} …", c.name) } else { c.name.clone() };
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
        let empty = self.panes.get(&id).is_some_and(|p| p.shell.at_prompt && p.shell.line == Some(0));
        self.follow_line(id, code, text, plain);
        let Some(s) = &self.slash else {
            // Only a `/` that is the first thing on an empty command line,
            // in a pane typed into alone.
            if plain && text == "/" && empty && self.input_targets(id).len() == 1 {
                self.slash = Some(Slash { pane: id, typed: String::new(), selected: 0, first: 0 });
                self.full = true;
                self.redraw();
            }
            return false;
        };
        if s.pane != id {
            self.close_slash();
            return false;
        }
        let n = self.slash_rows(&s.typed).len();
        // While it is open, the line is the slash and what follows it.
        let on_line = 1 + s.typed.chars().count();
        let keep_line = |app: &mut App| {
            if let Some(p) = app.panes.get_mut(&id) {
                p.shell.line = Some(on_line);
            }
        };
        match code {
            KeyCode::ArrowDown | KeyCode::ArrowUp if plain => {
                keep_line(self);
                if let Some(s) = &mut self.slash {
                    s.selected = if code == KeyCode::ArrowDown { (s.selected + 1).min(n.saturating_sub(1)) } else { s.selected.saturating_sub(1) };
                    s.first = s.first.min(s.selected).max((s.selected + 1).saturating_sub(SHOWN));
                }
                self.full = true;
                self.redraw();
                true
            }
            KeyCode::Escape => {
                keep_line(self);
                self.close_slash();
                true
            }
            KeyCode::Tab | KeyCode::Enter | KeyCode::NumpadEnter if plain && n > 0 => {
                self.slash_take(code == KeyCode::Tab, event_loop);
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
                // A second slash: a path after all.
                if text.contains('/') {
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

    /// How long the command line is, followed key by key: printing keys
    /// add, Backspace takes away, Ctrl U and Ctrl C empty it; any other
    /// key makes it unknown until the next prompt.
    fn follow_line(&mut self, id: PaneId, code: KeyCode, text: &str, plain: bool) {
        let ctrl = self.mods.control_key();
        let Some(p) = self.panes.get_mut(&id) else { return };
        let line = &mut p.shell.line;
        *line = match code {
            KeyCode::KeyU | KeyCode::KeyC if ctrl => Some(0),
            KeyCode::Backspace if plain => line.map(|n| n.saturating_sub(1)),
            _ if plain && !text.is_empty() && !text.chars().any(char::is_control) => line.map(|n| n + text.chars().count()),
            KeyCode::ShiftLeft | KeyCode::ShiftRight | KeyCode::ControlLeft | KeyCode::ControlRight | KeyCode::AltLeft | KeyCode::AltRight => *line,
            _ => None,
        };
    }

    /// After what is typed changed: back to the first row, or the list
    /// gone when nothing matches.
    fn slash_narrowed(&mut self) {
        let typed = self.slash.as_ref().map(|s| s.typed.clone()).unwrap_or_default();
        if self.slash_rows(&typed).is_empty() && self.slash_arg_command(&typed).is_none() {
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
    fn slash_take(&mut self, tab: bool, event_loop: &ActiveEventLoop) {
        let Some(s) = &self.slash else { return };
        let (id, typed, selected) = (s.pane, s.typed.clone(), s.selected);
        let rows = self.slash_rows(&typed);
        let Some(row) = rows.get(selected) else { return };
        let write = |app: &mut App, bytes: &[u8]| {
            if let Some(p) = app.panes.get(&id) {
                p.pty.write(bytes);
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
                p.shell.line = Some(1 + rest.chars().count());
            }
            if let Some(s) = &mut self.slash {
                s.typed = rest;
                s.selected = 0;
                s.first = 0;
            }
            self.full = true;
            return self.redraw();
        }
        let _ = tab;
        let label = row.label.clone();
        // The `/` and what follows it, rubbed out: the line empty again.
        if let Some(p) = self.panes.get_mut(&id) {
            p.shell.line = Some(0);
        }
        write(self, &vec![0x7f; typed.chars().count() + 1]);
        self.slash = None;
        self.full = true;
        let (name, arg) = label.split_once(' ').map(|(n, a)| (n.to_string(), a.to_string())).unwrap_or((label.clone(), String::new()));
        let Some(c) = self.slash_commands().into_iter().find(|c| c.name == name) else { return };
        match c.does {
            Does::Act(a) => self.run(a, event_loop),
            Does::Toggle(key) => self.settings_do(Hit::Toggle(key)),
            Does::Theme => {
                if let Some(i) = THEMES.iter().position(|t| t.name.to_lowercase().replace(' ', "-") == arg) {
                    self.settings_do(Hit::Theme(i));
                }
            }
            Does::Accent => {
                if let Some(i) = ACCENTS.iter().position(|(n, _)| *n == arg) {
                    self.settings_do(Hit::Accent(i));
                }
            }
            Does::Cd => {
                let roots: Vec<PathBuf> = self.config.projects.iter().map(|p| persist::expand(p)).collect();
                if let Some(dir) = palette::projects(&roots).into_iter().find(|d| folder_name(d) == arg) {
                    write(self, format!("cd '{}'\r", dir.display().to_string().replace('\'', "'\\''")).as_bytes());
                }
            }
            Does::Insert(text) => self.slash_insert(id, &text),
            Does::Run(cmd) => write(self, format!("{cmd}\r").as_bytes()),
            Does::Date => self.slash_insert(id, &date_today()),
            Does::Cwd => {
                let cwd = self.panes.get(&id).and_then(|p| p.pty.cwd()).map(|d| d.display().to_string()).unwrap_or_default();
                self.slash_insert(id, &cwd);
            }
        }
        self.redraw();
    }

    /// Text put on the command line, counted as typed.
    fn slash_insert(&mut self, id: PaneId, text: &str) {
        if let Some(p) = self.panes.get_mut(&id) {
            p.pty.write(text.as_bytes());
            p.shell.line = p.shell.line.map(|n| n + text.chars().count());
        }
    }

    /// The list, by the cursor: under it, or over it near the bottom.
    pub(super) fn draw_slash(&mut self) {
        let Some(s) = &self.slash else { return };
        let (id, typed, selected, first) = (s.pane, s.typed.clone(), s.selected, s.first);
        let Some(p) = self.panes.get(&id) else { return };
        if !p.rect.contains(p.content.x, p.content.y) {
            return;
        }
        let cur = p.term.cursor();
        let (cw, ch) = (self.fonts.cell_w, self.fonts.cell_h);
        let content = p.content;
        let rows = self.slash_rows(&typed);
        let c = self.cur_theme.chrome();
        let accent = self.cur_accent;
        let scale = self.scale;
        let px = |v: f32| (v * scale).round() as i32;
        let (uh, sh) = (self.fonts.ui.height, self.fonts.small.height);
        let row_h = uh + px(10.0);
        let shown: Vec<(usize, Row)> = rows.into_iter().enumerate().skip(first).take(SHOWN).collect();
        if shown.is_empty() {
            return;
        }
        let name_w = shown.iter().map(|(_, r)| self.fonts.ui.width(&format!("/{}", r.label))).max().unwrap_or(0);
        let what_w = shown.iter().map(|(_, r)| self.fonts.small.width(&r.what)).max().unwrap_or(0);
        let kind_w = shown.iter().map(|(_, r)| self.fonts.small.width(r.kind)).max().unwrap_or(0);
        let w = (px(12.0) + name_w + px(16.0) + what_w + px(16.0) + kind_w + px(12.0)).min(self.canvas.width - px(16.0));
        let h = shown.len() as i32 * row_h + px(8.0);
        // Its left at the slash, one column back from the cursor.
        let sx = content.x + (cur.x as i32 - 1 - typed.chars().count() as i32).max(0) * cw;
        let x = sx.min(self.canvas.width - w - px(8.0)).max(px(8.0));
        let below = content.y + (cur.y as i32 + 1) * ch + px(4.0);
        let y = if below + h <= self.canvas.height - px(8.0) { below } else { content.y + cur.y as i32 * ch - px(4.0) - h };
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
        Does::Toggle(_) | Does::Theme | Does::Accent => "setting",
        Does::Cd => "go",
        Does::Insert(_) | Does::Date | Does::Cwd => "insert",
        Does::Run(_) => "run",
    }
}

/// Today as YYYY-MM-DD, from the system's clock and zone.
fn date_today() -> String {
    std::process::Command::new("date").arg("+%F").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}
