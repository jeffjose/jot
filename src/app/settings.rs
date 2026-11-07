//! Settings, on Ctrl , or from the right-click menu and the palette: a
//! window over the panes, its sections as tabs down the left, each a page
//! of its own on the right that scrolls when it is long. Each setting is a row:
//! its name, a line on what it does, its control at the right, and Reset
//! when it is not as it started. What is changed is seen at once and
//! written to the config.

use super::*;
use crate::canvas::{mix, on};
use crate::font;
use crate::theme::{ACCENTS, THEMES};


const SECTIONS: [&str; 5] = ["Appearance", "Workspaces", "Fonts", "Keys", "About"];

/// A setting, for Reset.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Key {
    Theme,
    Accent,
    Family,
    Size,
    LineHeight,
    Contrast,
    UiFamily,
    UiSize,
    Captions,
    StatusLine,
    /// A workspace's theme, in the list of themes.
    WsTheme,
    Ligatures,
    CommandBars,
    WsColors,
    Bold,
}

const ALL_KEYS: [Key; 14] = [Key::Bold, Key::WsColors, Key::CommandBars, Key::Ligatures, Key::Theme, Key::Accent, Key::Contrast, Key::Family, Key::Size, Key::LineHeight, Key::UiFamily, Key::UiSize, Key::Captions, Key::StatusLine];

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hit {
    Section(usize),
    Theme(usize),
    Accent(usize),
    Toggle(Key),
    Step(Key, i32),
    Open(Key),
    Item(usize),
    Reset(Key),
    Thumb,
    Track,
    Close,
    /// A workspace's accent: an index into the accents, or none for the settings'.
    WsAccent(usize, usize),
    WsTheme(usize),
    /// A workspace's accent swatch, which opens its colors.
    WsAccentOpen(usize),
    /// A workspace's name, which a click lets you change.
    WsName(usize),
    WsGo(usize),
    /// Closing a workspace: once to ask, again to do it.
    WsClose(usize),
    WsNew,
    /// Not closing the workspace after all.
    WsKeep,
}

pub struct Dropdown {
    pub key: Key,
    /// For a workspace's theme, which workspace.
    pub ws: usize,
    pub items: Vec<String>,
    pub filter: crate::field::TextField,
    pub selected: usize,
    pub first: usize,
    pub anchor: Rect,
}

impl Dropdown {
    fn shown(&self) -> Vec<(usize, String)> {
        let f = self.filter.string().to_lowercase();
        self.items.iter().enumerate().filter(|(_, n)| n.to_lowercase().contains(&f)).map(|(i, n)| (i, n.clone())).collect()
    }
}

pub struct Settings {
    /// How far the page is scrolled, in pixels.
    pub scroll: i32,
    pub hover: Option<Hit>,
    /// What can be clicked, where it was last drawn.
    pub hits: Vec<(Rect, Hit)>,
    pub content_h: i32,
    pub view_h: i32,
    /// Where each section starts on the page.
    pub tops: Vec<i32>,
    /// The page shown: Appearance, Fonts, Keys or About.
    pub section: usize,
    pub dropdown: Option<Dropdown>,
    /// A workspace's colors open, under its swatch.
    pub accent_pop: Option<(usize, Rect)>,
    /// A workspace being renamed, and its name so far.
    pub renaming: Option<(usize, crate::field::TextField)>,
    /// A workspace whose close button has been clicked once.
    pub closing: Option<usize>,
    /// A color typed as hex in a workspace's colors.
    pub hex: crate::field::TextField,
    /// The scrollbar's thumb being dragged: where from, and the scroll then.
    pub drag: Option<(i32, i32)>,
    /// The installed fonts, read once: monospaced, and all.
    mono: Option<Vec<String>>,
    all: Option<Vec<String>>,
}

/// The red of something that cannot be undone.
const DANGER: u32 = 0xC9423A;

const KEYS: &[(&str, &str)] = &[
    ("Ctrl F", "Find in pane; Enter goes to the previous match"),
    ("Ctrl K", "Go to workspace, pane or folder"),
    ("Ctrl P", "Command palette"),
    ("Ctrl ,", "Settings"),
    ("Alt V", "Split right"),
    ("Alt S", "Split down"),
    ("Alt Shift V", "Split left"),
    ("Alt Shift S", "Split up"),
    ("Alt H J K L", "Move to the pane left, down, up or right"),
    ("Ctrl T", "New pane to the right"),
    ("Ctrl W  Alt X", "Close pane"),
    ("Ctrl Shift T", "Reopen closed pane"),
    ("Alt Shift F", "Focus pane / show all"),
    ("Alt Arrows", "Resize pane"),
    ("Alt =", "Equalize pane sizes"),
    ("Alt 1–9", "Go to a workspace"),
    ("Alt ,  Alt .", "Previous / next workspace"),
    ("Ctrl N  Alt Shift N", "New workspace, in this pane's folder"),
    ("Alt Shift M", "Multi: type into every pane; click a MULTI tag to skip one"),
    ("Alt ;", "Pane command line: run now, or queue behind the running command"),
    ("Ctrl R", "Restart the running command, or run the last again (Ctrl Shift R: kill it first)"),
    ("Alt P", "Copy mode: hjkl to move, v or V to select, y to copy"),
    ("Alt Shift O", "Move pane to a new window, and back"),
    ("Ctrl Shift ↑ ↓", "Previous / next prompt"),
    ("Ctrl Shift E", "Edit the command in a larger editor"),
    ("Alt `", "Toggle roaming terminal"),
    ("Ctrl Shift C", "Copy"),
    ("Ctrl Shift V", "Paste"),
    ("Shift PageUp", "Scroll up a page"),
    ("Ctrl =  -  0", "Zoom in, out, reset"),
];

fn config_key(key: Key) -> &'static str {
    match key {
        Key::Theme => "theme",
        Key::Accent => "accent",
        Key::Family => "font-family",
        Key::Size => "font-size",
        Key::LineHeight => "line-height",
        Key::Contrast => "min-contrast",
        Key::UiFamily => "ui-font-family",
        Key::UiSize => "ui-font-size",
        Key::Captions => "captions",
        Key::StatusLine => "status-line",
        Key::WsTheme => "theme",
        Key::Ligatures => "ligatures",
        Key::CommandBars => "command-bars",
        Key::WsColors => "workspace-colors",
        Key::Bold => "bold-text",
    }
}

impl App {
    pub(super) fn open_settings(&mut self) {
        self.palette = None;
        self.menu = None;
        self.settings = Some(Settings { scroll: 0, hover: None, hits: Vec::new(), content_h: 0, view_h: 1, tops: Vec::new(), section: 0, dropdown: None, accent_pop: None, renaming: None, closing: None, hex: crate::field::TextField::default(), drag: None, mono: None, all: None });
        self.full = true;
        self.redraw();
    }

    fn close_settings(&mut self) {
        self.settings = None;
        self.full = true;
        self.redraw();
        if let Some(w) = &self.window {
            w.set_cursor(CursorIcon::Text);
        }
    }

    fn modified(&self, key: Key) -> bool {
        let c = &self.config;
        match key {
            Key::Theme => !c.theme.eq_ignore_ascii_case(&persist::default_theme()),
            Key::Accent => crate::theme::accent(&c.accent) != crate::theme::accent(&persist::default_accent()),
            Key::Family => !c.font_family.eq_ignore_ascii_case(font::TERMINAL_DEFAULT),
            Key::Size => self.font_size != persist::default_font_size(),
            Key::LineHeight => (c.line_height - persist::default_line_height()).abs() > 0.001,
            Key::Contrast => (c.min_contrast - persist::default_min_contrast()).abs() > 0.001,
            Key::UiFamily => !c.ui_font_family.eq_ignore_ascii_case(font::INTERFACE_DEFAULT),
            Key::UiSize => c.ui_font_size != persist::default_ui_font_size(),
            Key::Captions => !c.captions,
            Key::StatusLine => c.status_line,
            Key::WsTheme => false,
            Key::Ligatures => !c.ligatures,
            Key::CommandBars => c.command_bars,
            Key::WsColors => !c.workspace_colors,
            Key::Bold => !c.bold_text,
        }
    }

    pub(super) fn set_theme(&mut self, i: usize) {
        let Some(theme) = THEMES.get(i) else { return };
        self.theme = theme;
        self.config.theme = theme.name.to_string();
        // Each pane takes it, unless its workspace has a theme of its own,
        // as the next frame is drawn.
        self.full = true;
        persist::set_config_value("theme", &format!("{:?}", theme.name));
    }

    fn set_family(&mut self, key: Key, name: String) {
        match key {
            Key::Family => self.config.font_family = name.clone(),
            _ => self.config.ui_font_family = name.clone(),
        }
        persist::set_config_value(config_key(key), &format!("{name:?}"));
        self.reload_fonts();
    }

    /// A setting back as it started, its line in the config commented out.
    fn reset(&mut self, key: Key) {
        match key {
            Key::Theme => {
                let i = THEMES.iter().position(|t| t.name == persist::default_theme()).unwrap_or(0);
                self.set_theme(i);
            }
            Key::Accent => {
                self.config.accent = persist::default_accent();
                self.accent = crate::theme::accent(&self.config.accent);
            }
            Key::Family => {
                self.config.font_family = persist::default_font_family();
                self.reload_fonts();
            }
            Key::UiFamily => {
                self.config.ui_font_family = persist::default_ui_font_family();
                self.reload_fonts();
            }
            Key::Size => {
                self.config.font_size = persist::default_font_size();
                self.set_font_size(self.config.font_size);
            }
            Key::Contrast => {
                self.config.min_contrast = persist::default_min_contrast();
                self.refresh_panes();
            }
            Key::LineHeight => {
                self.config.line_height = persist::default_line_height();
                self.reload_fonts();
            }
            Key::UiSize => {
                self.config.ui_font_size = persist::default_ui_font_size();
                self.reload_fonts();
            }
            Key::Captions => {
                self.config.captions = true;
                self.relayout();
            }
            Key::WsTheme => {}
            Key::Bold => {
                self.config.bold_text = true;
                self.reload_fonts();
            }
            Key::WsColors => {
                self.config.workspace_colors = true;
                self.retint();
            }
            Key::CommandBars => {
                self.config.command_bars = false;
                self.refresh_panes();
            }
            Key::Ligatures => {
                self.config.ligatures = true;
                self.reload_fonts();
            }
            Key::StatusLine => {
                self.config.status_line = false;
                self.relayout();
            }
        }
        persist::unset_config_value(config_key(key));
    }

    /// A workspace's name, made a field with all of it selected.
    fn start_rename(&mut self, wi: usize) {
        self.finish_rename(true);
        let Some(name) = self.workspaces.get(wi).map(|w| w.name.clone()) else { return };
        let mut field = crate::field::TextField::new(&name);
        field.anchor = Some(0);
        if let Some(s) = &mut self.settings {
            s.renaming = Some((wi, field));
        }
    }

    /// The name typed kept, or let go of; an empty one is not kept.
    fn finish_rename(&mut self, keep: bool) {
        let Some((wi, field)) = self.settings.as_mut().and_then(|s| s.renaming.take()) else { return };
        let name = field.string().trim().to_string();
        if keep && !name.is_empty() {
            if let Some(ws) = self.workspaces.get_mut(wi) {
                ws.name = name;
                self.unsaved = true;
                self.set_title();
            }
        }
        self.full = true;
        self.redraw();
    }

    pub(super) fn settings_do(&mut self, hit: Hit) {
        match hit {
            Hit::Section(i) => {
                if let Some(s) = &mut self.settings {
                    s.section = i;
                    s.scroll = 0;
                    s.dropdown = None;
                }
            }
            Hit::Theme(i) => self.set_theme(i),
            Hit::Accent(i) => {
                let (name, color) = ACCENTS[i];
                self.accent = color;
                self.config.accent = name.to_string();
                persist::set_config_value("accent", &format!("{name:?}"));
            }
            Hit::Toggle(key) => {
                let on = match key {
                    Key::Captions => {
                        self.config.captions = !self.config.captions;
                        self.config.captions
                    }
                    Key::Bold => {
                        self.config.bold_text = !self.config.bold_text;
                        self.config.bold_text
                    }
                    Key::WsColors => {
                        self.config.workspace_colors = !self.config.workspace_colors;
                        self.config.workspace_colors
                    }
                    Key::CommandBars => {
                        self.config.command_bars = !self.config.command_bars;
                        self.config.command_bars
                    }
                    Key::Ligatures => {
                        self.config.ligatures = !self.config.ligatures;
                        self.config.ligatures
                    }
                    _ => {
                        self.config.status_line = !self.config.status_line;
                        self.config.status_line
                    }
                };
                persist::set_config_value(config_key(key), if on { "true" } else { "false" });
                if key == Key::WsColors {
                    self.retint();
                }
                if key == Key::CommandBars {
                    self.refresh_panes();
                }
                if key == Key::Ligatures || key == Key::Bold {
                    self.reload_fonts();
                }
                self.relayout();
            }
            Hit::Step(Key::Size, by) => {
                self.set_font_size(self.font_size + by as f32);
                self.config.font_size = self.font_size;
                persist::set_config_value("font-size", &format!("{}", self.font_size));
            }
            Hit::Step(Key::Contrast, by) => {
                let v = ((self.config.min_contrast * 2.0).round() + by as f32) / 2.0;
                self.config.min_contrast = v.clamp(1.0, 21.0);
                persist::set_config_value("min-contrast", &format!("{:.1}", self.config.min_contrast));
                self.refresh_panes();
            }
            Hit::Step(Key::LineHeight, by) => {
                let v = ((self.config.line_height * 20.0).round() + by as f32) / 20.0;
                self.config.line_height = v.clamp(0.8, 2.0);
                persist::set_config_value("line-height", &format!("{:.2}", self.config.line_height));
                self.reload_fonts();
            }
            Hit::Step(_, by) => {
                self.config.ui_font_size = (self.config.ui_font_size + by as f32).clamp(8.0, 32.0);
                persist::set_config_value("ui-font-size", &format!("{}", self.config.ui_font_size));
                self.reload_fonts();
            }
            Hit::Open(key) => {
                let anchor = self.settings.as_ref().and_then(|s| s.hits.iter().find(|(_, h)| *h == hit).map(|(r, _)| *r)).unwrap_or_default();
                let current = if key == Key::Family { self.config.font_family.clone() } else { self.config.ui_font_family.clone() };
                if let Some(s) = &mut self.settings {
                    let items = if key == Key::Family { s.mono.get_or_insert_with(|| font::families(true)).clone() } else { s.all.get_or_insert_with(|| font::families(false)).clone() };
                    let selected = items.iter().position(|n| n.eq_ignore_ascii_case(&current)).unwrap_or(0);
                    s.dropdown = Some(Dropdown { key, ws: 0, items, filter: crate::field::TextField::default(), selected, first: selected.saturating_sub(4), anchor });
                }
            }
            Hit::Item(i) => {
                let picked = self.settings.as_mut().and_then(|s| s.dropdown.take()).and_then(|d| d.items.get(i).cloned().map(|n| (d.key, d.ws, i, n)));
                match picked {
                    // The first of a workspace's themes is the settings' own.
                    Some((Key::WsTheme, wi, i, name)) => {
                        if let Some(ws) = self.workspaces.get_mut(wi) {
                            ws.theme = (i > 0).then_some(name);
                        }
                        self.unsaved = true;
                        self.full = true;
                    }
                    Some((key, _, _, name)) => self.set_family(key, name),
                    None => {}
                }
            }
            Hit::Reset(key) => self.reset(key),
            Hit::Close => return self.close_settings(),
            Hit::WsAccentOpen(wi) => {
                let typed = self.workspaces.get(wi).and_then(|w| w.accent.clone()).filter(|a| a.starts_with('#')).map(|a| a[1..].to_string()).unwrap_or_default();
                let anchor = self.settings.as_ref().and_then(|s| s.hits.iter().find(|(_, h)| *h == hit).map(|(r, _)| *r)).unwrap_or_default();
                if let Some(s) = &mut self.settings {
                    s.accent_pop = if s.accent_pop.is_some_and(|(w, _)| w == wi) { None } else { Some((wi, anchor)) };
                    s.hex = crate::field::TextField::new(&typed);
                    s.hex.anchor = Some(0);
                }
            }
            Hit::WsAccent(wi, i) => {
                if let Some(s) = &mut self.settings {
                    s.accent_pop = None;
                }
                if let Some(ws) = self.workspaces.get_mut(wi) {
                    ws.accent = ACCENTS.get(i).map(|(n, _)| n.to_string());
                }
                self.unsaved = true;
            }
            Hit::WsName(wi) => {
                if self.settings.as_ref().is_some_and(|s| s.renaming.as_ref().is_some_and(|(r, _)| *r == wi)) {
                    return;
                }
                self.start_rename(wi);
            }
            Hit::WsGo(wi) => self.go_to(wi),
            Hit::WsKeep => {}
            // The first click asks; the second, in settings_click, closes.
            Hit::WsClose(wi) => {
                if let Some(s) = &mut self.settings {
                    s.closing = Some(wi);
                }
            }
            Hit::WsNew => {
                self.new_workspace();
                self.start_rename(self.workspaces.len() - 1);
            }
            Hit::WsTheme(wi) => {
                let anchor = self.settings.as_ref().and_then(|s| s.hits.iter().find(|(_, h)| *h == hit).map(|(r, _)| *r)).unwrap_or_default();
                let current = self.workspaces.get(wi).and_then(|w| w.theme.clone());
                let mut items = vec!["Default theme".to_string()];
                items.extend(THEMES.iter().map(|t| t.name.to_string()));
                let selected = current.and_then(|c| items.iter().position(|n| *n == c)).unwrap_or(0);
                if let Some(s) = &mut self.settings {
                    s.dropdown = Some(Dropdown { key: Key::WsTheme, ws: wi, items, filter: crate::field::TextField::default(), selected, first: selected.saturating_sub(4), anchor });
                }
            }
            Hit::Thumb | Hit::Track => {}
        }
        self.full = true;
        self.redraw();
    }

    fn scroll_by(&mut self, by: i32) {
        if let Some(s) = &mut self.settings {
            s.scroll = (s.scroll + by).clamp(0, (s.content_h - s.view_h).max(0));
            self.full = true;
            self.redraw();
        }
    }

    pub(super) fn settings_key(&mut self, event: &winit::event::KeyEvent) {
        let PhysicalKey::Code(code) = event.physical_key else { return };
        let page = self.settings.as_ref().map_or(400, |s| s.view_h * 9 / 10);
        let step = self.px(48.0);
        // With a name being typed, the keys are its.
        if self.settings.as_ref().is_some_and(|s| s.renaming.is_some()) {
            match code {
                KeyCode::Enter | KeyCode::NumpadEnter => return self.finish_rename(true),
                KeyCode::Escape => return self.finish_rename(false),
                _ => {}
            }
            let pasted = if self.mods.control_key() && code == KeyCode::KeyV { self.clip_get(false) } else { None };
            let copied: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
            let mods = self.mods;
            if let Some((_, field)) = self.settings.as_mut().and_then(|s| s.renaming.as_mut()) {
                let copied = &copied;
                field.key(code, mods, event.text.as_deref(), &mut || pasted.clone(), &mut |t| *copied.borrow_mut() = Some(t));
            }
            if let Some(s) = copied.into_inner() {
                self.clip_set(s, false);
            }
            self.full = true;
            return self.redraw();
        }
        // With a workspace's colors open, the keys go to the hex box.
        if let Some((wi, _)) = self.settings.as_ref().and_then(|s| s.accent_pop) {
            match code {
                KeyCode::Escape => {
                    if let Some(s) = &mut self.settings {
                        s.accent_pop = None;
                    }
                }
                KeyCode::Enter | KeyCode::NumpadEnter => {
                    let typed = self.settings.as_ref().map(|s| s.hex.string()).unwrap_or_default();
                    if let Some(color) = hex_color(&typed) {
                        if let Some(ws) = self.workspaces.get_mut(wi) {
                            ws.accent = Some(format!("#{color:06x}"));
                        }
                        if let Some(s) = &mut self.settings {
                            s.accent_pop = None;
                        }
                        self.unsaved = true;
                    }
                }
                _ => {
                    let pasted = if self.mods.control_key() && code == KeyCode::KeyV { self.clip_get(false) } else { None };
                    let mods = self.mods;
                    if let Some(s) = &mut self.settings {
                        s.hex.key(code, mods, event.text.as_deref(), &mut || pasted.clone(), &mut |_| {});
                    }
                }
            }
            self.full = true;
            return self.redraw();
        }
        // With a font list open, the keys are its.
        let list_open = self.settings.as_ref().is_some_and(|s| s.dropdown.is_some());
        if list_open {
            let mut pick = None;
            let pasted = if self.mods.control_key() && code == KeyCode::KeyV { self.clip_get(false) } else { None };
            let copied: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
            if let Some(s) = &mut self.settings {
                let Some(d) = &mut s.dropdown else { return };
                let n = d.shown().len();
                match code {
                    KeyCode::Escape => s.dropdown = None,
                    KeyCode::Enter | KeyCode::NumpadEnter => pick = d.shown().get(d.selected).map(|(i, _)| *i),
                    KeyCode::ArrowUp => d.selected = d.selected.saturating_sub(1),
                    KeyCode::ArrowDown => d.selected = (d.selected + 1).min(n.saturating_sub(1)),
                    KeyCode::PageUp => d.selected = d.selected.saturating_sub(10),
                    KeyCode::PageDown => d.selected = (d.selected + 10).min(n.saturating_sub(1)),
                    _ => {
                        let mods = self.mods;
                        let copied = &copied;
                        let out = d.filter.key(code, mods, event.text.as_deref(), &mut || pasted.clone(), &mut |t| *copied.borrow_mut() = Some(t));
                        if out == crate::field::Outcome::Changed {
                            d.selected = 0;
                        }
                    }
                }
            }
            if let Some(s) = copied.into_inner() {
                self.clip_set(s, false);
            }
            if let Some(i) = pick {
                return self.settings_do(Hit::Item(i));
            }
            self.full = true;
            return self.redraw();
        }
        match code {
            KeyCode::Escape if self.settings.as_ref().is_some_and(|s| s.accent_pop.is_some()) => {
                if let Some(s) = &mut self.settings {
                    s.accent_pop = None;
                }
                self.full = true;
                self.redraw();
            }
            KeyCode::Escape => self.close_settings(),
            KeyCode::ArrowUp => self.scroll_by(-step),
            KeyCode::ArrowDown => self.scroll_by(step),
            KeyCode::PageUp => self.scroll_by(-page),
            KeyCode::PageDown | KeyCode::Space => self.scroll_by(page),
            KeyCode::Home => self.scroll_by(-1_000_000),
            KeyCode::End => self.scroll_by(1_000_000),
            KeyCode::Tab => {
                let Some(s) = &self.settings else { return };
                let now = s.section;
                let n = SECTIONS.len();
                let next = if self.mods.shift_key() { (now + n - 1) % n } else { (now + 1) % n };
                self.settings_do(Hit::Section(next));
            }
            _ => {}
        }
    }

    fn settings_rect(&self) -> Rect {
        let (w, h) = (self.canvas.width, self.canvas.height);
        let width = self.px(940.0).min(w - 2 * self.px(24.0));
        let height = self.px(660.0).min(h - 2 * self.px(24.0));
        Rect { x: (w - width) / 2, y: (h - height) / 2, w: width, h: height }
    }

    fn settings_hit(&self, x: i32, y: i32) -> Option<Hit> {
        self.settings.as_ref()?.hits.iter().rev().find(|(r, _)| r.contains(x, y)).map(|(_, h)| *h)
    }

    /// A click while the settings are open: theirs, wherever it is. A
    /// click outside them closes them; outside a font list, the list.
    pub(super) fn settings_click(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if self.settings.is_none() {
            return false;
        }
        let (x, y) = self.pointer;
        let hit = self.settings_hit(x, y);
        // A click off the name being typed keeps what was typed.
        if self.settings.as_ref().is_some_and(|s| s.renaming.as_ref().is_some_and(|(r, _)| hit != Some(Hit::WsName(*r)))) {
            self.finish_rename(true);
        }
        // A second click on a workspace's Close closes it; any other click
        // forgets the first.
        if let Some(wi) = self.settings.as_mut().and_then(|s| s.closing.take()) {
            if hit == Some(Hit::WsClose(wi)) {
                let mut ids = Vec::new();
                if let Some(ws) = self.workspaces.get(wi) {
                    ws.root.panes(&mut ids);
                }
                for id in ids {
                    self.close(id, event_loop);
                }
                self.full = true;
                self.redraw();
                return true;
            }
            self.full = true;
            self.redraw();
        }
        let list_open = self.settings.as_ref().is_some_and(|s| s.dropdown.is_some());
        // With the colors open, a click picks one, or closes them.
        if self.settings.as_ref().is_some_and(|s| s.accent_pop.is_some()) {
            match hit {
                Some(h @ Hit::WsAccent(..)) => self.settings_do(h),
                Some(Hit::Track) => {}
                _ => {
                    if let Some(s) = &mut self.settings {
                        s.accent_pop = None;
                    }
                    self.full = true;
                    self.redraw();
                }
            }
            return true;
        }
        match hit {
            Some(Hit::Item(i)) => self.settings_do(Hit::Item(i)),
            Some(Hit::Track) if list_open => {}
            _ if list_open => {
                if let Some(s) = &mut self.settings {
                    s.dropdown = None;
                }
                self.full = true;
                self.redraw();
            }
            Some(Hit::Thumb) => {
                if let Some(s) = &mut self.settings {
                    s.drag = Some((y, s.scroll));
                }
            }
            Some(Hit::Track) => {
                let page = self.settings.as_ref().map_or(0, |s| s.view_h * 9 / 10);
                let thumb_y = self.settings.as_ref().and_then(|s| s.hits.iter().find(|(_, h)| *h == Hit::Thumb).map(|(r, _)| r.y)).unwrap_or(0);
                self.scroll_by(if y < thumb_y { -page } else { page });
            }
            Some(hit) => self.settings_do(hit),
            None if !self.settings_rect().contains(x, y) => self.close_settings(),
            None => {}
        }
        true
    }

    pub(super) fn settings_release(&mut self) {
        if let Some(s) = &mut self.settings {
            s.drag = None;
        }
    }

    pub(super) fn settings_hover(&mut self, x: i32, y: i32) -> bool {
        let Some(s) = &self.settings else { return false };
        if let Some((from_y, from_scroll)) = s.drag {
            let ratio = s.content_h as f32 / s.view_h.max(1) as f32;
            let max = (s.content_h - s.view_h).max(0);
            let to = (from_scroll + ((y - from_y) as f32 * ratio) as i32).clamp(0, max);
            if let Some(s) = &mut self.settings {
                s.scroll = to;
            }
            self.full = true;
            self.redraw();
            return true;
        }
        let hit = self.settings_hit(x, y);
        if let Some(s) = &mut self.settings {
            if s.hover != hit {
                s.hover = hit;
                self.full = true;
                self.redraw();
            }
        }
        if let Some(w) = &self.window {
            w.set_cursor(if matches!(hit, Some(h) if h != Hit::Track) { CursorIcon::Pointer } else { CursorIcon::Default });
        }
        true
    }

    pub(super) fn settings_scroll(&mut self, lines: isize) -> bool {
        if self.settings.is_none() {
            return false;
        }
        if let Some(d) = self.settings.as_mut().and_then(|s| s.dropdown.as_mut()) {
            let n = d.shown().len();
            d.selected = (d.selected as isize + lines.signum() * 3).clamp(0, n.saturating_sub(1) as isize) as usize;
            self.full = true;
            self.redraw();
            return true;
        }
        let by = lines as i32 * self.px(16.0);
        self.scroll_by(by);
        true
    }

    pub(super) fn draw_settings(&mut self) {
        let c = self.theme.chrome();
        let Some(mut st) = self.settings.take() else { return };
        let scale = self.scale;
        let px = |v: f32| (v * scale).round() as i32;
        let r = self.settings_rect();
        let accent = self.accent;
        let accent_text = mix(accent, 0xFFFFFF, 110);
        let mut hits: Vec<(Rect, Hit)> = Vec::new();
        let hover = st.hover;
        let (renaming, closing) = (st.renaming.clone(), st.closing);
        let modified: Vec<Key> = ALL_KEYS.into_iter().filter(|k| self.modified(*k)).collect();
        let current_theme = THEMES.iter().position(|t| std::ptr::eq(t, self.theme));
        let current_accent = ACCENTS.iter().position(|(_, c)| *c == self.accent);
        let (family, size, ui_family, ui_size, line_height) = (self.config.font_family.clone(), self.font_size, self.config.ui_font_family.clone(), self.config.ui_font_size, self.config.line_height);
        let (captions, status_line) = (self.config.captions, self.config.status_line);
        let ws_rows: Vec<(String, Option<String>, Option<String>, bool)> = self.workspaces.iter().enumerate().map(|(i, w)| (w.name.clone(), w.accent.clone(), w.theme.clone(), i == self.current)).collect();
        let (ws_given, ws_colors) = (self.ws_accents(), self.config.workspace_colors);
        let min_contrast = self.config.min_contrast;
        let ligatures = self.config.ligatures;
        let bold_text = self.config.bold_text;
        let command_bars = self.config.command_bars;
        let version = crate::version();
        let canvas_h = self.canvas.height;
        let canvas = &mut self.canvas;
        let fonts = &mut self.fonts;
        let (uh, sh_, hh) = (fonts.ui.height, fonts.small.height, fonts.heading.height);

        canvas.panel(r, px(12.0), c.panel, c.edge);

        // Down the left: the sections, the one in view picked out.
        let side_w = px(210.0).min(r.w / 3);
        let side = Rect { x: r.x + 1, y: r.y + 1, w: side_w, h: r.h - 2 };
        canvas.round_fill(side, px(11.0), c.side, 255);
        canvas.fill(Rect { x: side.x + side.w - px(12.0), y: side.y, w: px(12.0), h: side.h }, c.side);
        canvas.fill(Rect { x: side.x + side.w, y: side.y, w: 1, h: side.h }, c.edge);
        let pad = px(20.0);
        canvas.text(&mut fonts.heading, "Settings", (side.x + pad) as f32, side.y + pad, c.text, side);
        let in_view = st.section;
        let section = st.section;
        let nav_h = uh + px(16.0);
        let mut ny = side.y + pad + hh + pad;
        for (i, name) in SECTIONS.iter().enumerate() {
            let row = Rect { x: side.x + px(10.0), y: ny, w: side.w - px(20.0), h: nav_h };
            if i == in_view {
                canvas.round_fill(row, px(7.0), accent, 255);
            } else if hover == Some(Hit::Section(i)) {
                canvas.round_fill(row, px(7.0), c.field, 255);
            }
            let fg = if i == in_view { on(accent) } else { c.text };
            canvas.text(&mut fonts.ui, name, (row.x + px(12.0)) as f32, row.y + (nav_h - uh) / 2, fg, row);
            hits.push((row, Hit::Section(i)));
            ny += nav_h + px(2.0);
        }

        // At the top right: close, an × drawn from two strokes.
        let cs = px(30.0);
        let close = Rect { x: r.x + r.w - cs - px(12.0), y: r.y + px(12.0), w: cs, h: cs };
        if hover == Some(Hit::Close) {
            canvas.round_fill(close, cs / 2, c.hover, 255);
        }
        let arm = px(5.0);
        let (cx, cy) = (close.x + cs / 2, close.y + cs / 2);
        let stroke = if hover == Some(Hit::Close) { c.text } else { c.dim };
        let dot = px(2.0).max(2);
        for t in -arm..=arm {
            canvas.round_fill(Rect { x: cx + t - dot / 2, y: cy + t - dot / 2, w: dot, h: dot }, dot / 2, stroke, 255);
            canvas.round_fill(Rect { x: cx + t - dot / 2, y: cy - t - dot / 2, w: dot, h: dot }, dot / 2, stroke, 255);
        }

        // On the right, the page, scrolled, kept inside its area.
        let view = Rect { x: side.x + side.w + 1, y: r.y + px(8.0), w: r.x + r.w - (side.x + side.w + 1) - px(18.0), h: r.h - px(16.0) };
        let body = Rect { x: view.x + px(32.0), y: view.y, w: view.w - px(64.0) - cs, h: view.h };
        let right = body.x + body.w;
        canvas.clip = Some(view);
        let mut y = view.y + px(16.0) - st.scroll;
        let mut tops = Vec::new();
        let row_pad = px(14.0);
        let mut page: Vec<(Rect, Hit)> = Vec::new();
        let field_h = uh + px(14.0);
        let row_h = uh + px(4.0) + sh_ + 2 * row_pad;
        let rule = mix(c.panel, c.edge, 160);
        let scroll = st.scroll;

        // A heading for each section.
        let heading = |canvas: &mut Canvas, fonts: &mut font::Fonts, y: &mut i32, tops: &mut Vec<i32>, name: &str| {
            tops.push(*y + scroll - view.y - px(16.0));
            canvas.text(&mut fonts.heading, name, body.x as f32, *y + px(8.0), c.text, view);
            *y += hh + px(20.0);
        };
        // A setting's name and what it does, with Reset after the name when
        // it has been changed. Returns the middle of the row, for its control.
        let title = |canvas: &mut Canvas, fonts: &mut font::Fonts, page: &mut Vec<(Rect, Hit)>, y: i32, name: &str, what: &str, key: Key| {
            let end = canvas.text(&mut fonts.ui, name, body.x as f32, y + row_pad, c.text, view);
            if modified.contains(&key) {
                let rx = end + px(10.0);
                let rw = fonts.small.width("Reset");
                let color = if hover == Some(Hit::Reset(key)) { c.text } else { accent_text };
                canvas.text(&mut fonts.small, "Reset", rx as f32, y + row_pad + (uh - sh_) / 2, color, view);
                page.push((Rect { x: rx - px(4.0), y: y + row_pad - px(2.0), w: rw + px(8.0), h: uh + px(4.0) }, Hit::Reset(key)));
            }
            canvas.text(&mut fonts.small, what, body.x as f32, y + row_pad + uh + px(4.0), c.dim, view);
            y + row_h / 2
        };
        let stepper = |canvas: &mut Canvas, fonts: &mut font::Fonts, page: &mut Vec<(Rect, Hit)>, mid: i32, key: Key, value: &str| {
            let b = field_h;
            let plus = Rect { x: right - b, y: mid - b / 2, w: b, h: b };
            let vw = px(52.0);
            let minus = Rect { x: plus.x - vw - b, y: plus.y, w: b, h: b };
            for (rect, by, glyph) in [(minus, -1, "−"), (plus, 1, "+")] {
                let hit = Hit::Step(key, by);
                canvas.round_fill(rect, px(7.0), if hover == Some(hit) { c.hover } else { c.field }, 255);
                canvas.ring(rect, px(7.0), 1, c.edge);
                let gw = fonts.ui.width(glyph);
                canvas.text(&mut fonts.ui, glyph, (rect.x + (b - gw) / 2) as f32, rect.y + (b - uh) / 2, c.text, rect);
                page.push((rect, hit));
            }
            let w = fonts.ui.width(value);
            canvas.text(&mut fonts.ui, value, (minus.x + b + (vw - w) / 2) as f32, mid - uh / 2, c.text, view);
        };

        // Each section is a page of its own.
        if section == 0 {
        // ---- Appearance ----
        heading(canvas, fonts, &mut y, &mut tops, "Appearance");
        title(canvas, fonts, &mut page, y, "Color theme", "Colors for the terminal and the window around it.", Key::Theme);
        y += row_h - row_pad / 2;
        for dark in [true, false] {
            canvas.text(&mut fonts.small, if dark { "DARK" } else { "LIGHT" }, body.x as f32, y, c.faint, view);
            y += sh_ + px(10.0);
            let gap = px(12.0);
            let cols = ((body.w + gap) / (px(180.0) + gap)).max(2);
            let card_w = (body.w - gap * (cols - 1)) / cols;
            let card_h = px(78.0);
            let group: Vec<(usize, &crate::theme::Theme)> = THEMES.iter().enumerate().filter(|(_, t)| t.dark == dark).collect();
            for (k, (i, t)) in group.iter().enumerate() {
                let (col, rowi) = (k as i32 % cols, k as i32 / cols);
                let card = Rect { x: body.x + col * (card_w + gap), y: y + rowi * (card_h + gap), w: card_w, h: card_h };
                canvas.round_fill(card, px(8.0), t.bg, 255);
                // A few lines of code in the theme's colors.
                let bar_h = px(5.0);
                let lines: [&[(u32, f32)]; 3] = [&[(t.ansi[4], 26.0), (t.fg, 46.0)], &[(t.ansi[2], 18.0), (t.ansi[3], 34.0), (t.fg, 14.0)], &[(t.ansi[5], 14.0), (t.fg, 40.0), (t.ansi[1], 16.0)]];
                for (li, segs) in lines.iter().enumerate() {
                    let mut bx = card.x + px(12.0);
                    let by = card.y + px(12.0) + li as i32 * px(11.0);
                    for (c, w) in segs.iter() {
                        let w = px(*w);
                        canvas.round_fill(Rect { x: bx, y: by, w, h: bar_h }, bar_h / 2, *c, 255);
                        bx += w + px(5.0);
                    }
                }
                canvas.text(&mut fonts.small, t.name, (card.x + px(12.0)) as f32, card.y + card_h - sh_ - px(10.0), t.fg, card);
                if Some(*i) == current_theme {
                    canvas.ring(Rect { x: card.x - px(3.0), y: card.y - px(3.0), w: card.w + px(6.0), h: card.h + px(6.0) }, px(10.0), px(2.0), accent);
                } else if hover == Some(Hit::Theme(*i)) {
                    canvas.ring(card, px(8.0), 1, mix(t.bg, t.fg, 110));
                } else {
                    canvas.ring(card, px(8.0), 1, mix(c.panel, c.edge, 200));
                }
                page.push((card, Hit::Theme(*i)));
            }
            let rows = (group.len() as i32 + cols - 1) / cols;
            y += rows * (card_h + gap) + px(12.0);
        }
        canvas.fill(Rect { x: body.x, y, w: body.w, h: 1 }, rule);

        title(canvas, fonts, &mut page, y, "Accent", "Highlight color for the focused pane, menus and switches.", Key::Accent);
        y += row_h - row_pad / 2;
        {
            let d = px(24.0);
            let gap = px(10.0);
            let per_row = ((body.w + gap) / (d + gap)).max(1);
            for (i, (_, color)) in ACCENTS.iter().enumerate() {
                let (col, rowi) = (i as i32 % per_row, i as i32 / per_row);
                let dot = Rect { x: body.x + px(4.0) + col * (d + gap), y: y + rowi * (d + gap), w: d, h: d };
                canvas.round_fill(dot, d / 2, *color, 255);
                if Some(i) == current_accent {
                    canvas.ring(Rect { x: dot.x - px(4.0), y: dot.y - px(4.0), w: d + px(8.0), h: d + px(8.0) }, d, px(2.0), c.text);
                } else if hover == Some(Hit::Accent(i)) {
                    canvas.ring(Rect { x: dot.x - px(3.0), y: dot.y - px(3.0), w: d + px(6.0), h: d + px(6.0) }, d, 1, c.dim);
                }
                page.push((dot, Hit::Accent(i)));
            }
            let rows = (ACCENTS.len() as i32 + per_row - 1) / per_row;
            y += rows * (d + gap) + px(14.0);
        }
        canvas.fill(Rect { x: body.x, y, w: body.w, h: 1 }, rule);

        {
            let mid = title(canvas, fonts, &mut page, y, "Minimum contrast", "Adjusts text that is hard to read on its background. VS Code uses 4.5; 1 turns it off.", Key::Contrast);
            stepper(canvas, fonts, &mut page, mid, Key::Contrast, &format!("{min_contrast:.1}"));
            y += row_h;
            canvas.fill(Rect { x: body.x, y, w: body.w, h: 1 }, rule);
        }
        for (name, what, key, value) in [
            ("Pane captions", "Each pane's title along its bottom edge.", Key::Captions, captions),
            ("Status line", "Workspaces, host and clock along the bottom of the window.", Key::StatusLine, status_line),
            ("Command bars", "A green or red bar beside each command for success or failure, with the time slow ones took.", Key::CommandBars, command_bars),
        ] {
            let mid = title(canvas, fonts, &mut page, y, name, what, key);
            let (sw, sh) = (px(40.0), px(22.0));
            let pill = Rect { x: right - sw, y: mid - sh / 2, w: sw, h: sh };
            canvas.round_fill(pill, sh / 2, if value { accent } else { c.hover }, 255);
            let k = sh - px(6.0);
            let kx = if value { pill.x + sw - k - px(3.0) } else { pill.x + px(3.0) };
            canvas.round_fill(Rect { x: kx, y: pill.y + px(3.0), w: k, h: k }, k / 2, 0xFFFFFF, 255);
            page.push((Rect { x: pill.x - px(4.0), y: pill.y - px(4.0), w: sw + px(8.0), h: sh + px(8.0) }, Hit::Toggle(key)));
            y += row_h;
            canvas.fill(Rect { x: body.x, y, w: body.w, h: 1 }, rule);
        }
        y += px(32.0);

        }
        if section == 1 {
        heading(canvas, fonts, &mut y, &mut tops, "Workspaces");
        canvas.text(&mut fonts.small, "Click a name to rename it. Default uses the theme and accent from Appearance.", body.x as f32, y - px(8.0), c.dim, view);
        y += sh_ + px(12.0);
        {
            let mid = title(canvas, fonts, &mut page, y, "Color each workspace", "Every workspace after the first gets its own accent, chosen by name. Pick one to override it.", Key::WsColors);
            let (sw, sh) = (px(40.0), px(22.0));
            let pill = Rect { x: right - sw, y: mid - sh / 2, w: sw, h: sh };
            canvas.round_fill(pill, sh / 2, if ws_colors { accent } else { c.hover }, 255);
            let k = sh - px(6.0);
            let kx = if ws_colors { pill.x + sw - k - px(3.0) } else { pill.x + px(3.0) };
            canvas.round_fill(Rect { x: kx, y: pill.y + px(3.0), w: k, h: k }, k / 2, 0xFFFFFF, 255);
            page.push((Rect { x: pill.x - px(4.0), y: pill.y - px(4.0), w: sw + px(8.0), h: sh + px(8.0) }, Hit::Toggle(Key::WsColors)));
            y += row_h + px(16.0);
        }
        // The workspaces as a list in a box, a row each, its controls in
        // columns at the right, and a row at the end for a new one.
        let many = ws_rows.len() > 1;
        let rh = px(52.0);
        let group = Rect { x: body.x, y, w: body.w, h: rh * (ws_rows.len() as i32 + 1) };
        canvas.round_fill(group, px(10.0), c.side, 255);
        let (gap, ib) = (px(10.0), px(30.0));
        let x_close = group.x + group.w - gap - ib;
        let x_go = x_close - px(4.0) - ib;
        let tw = px(180.0).min(body.w / 4);
        let x_theme = x_go - px(10.0) - tw;
        let aw = px(110.0);
        let x_acc = x_theme - px(8.0) - aw;
        let dot = px(2.0).max(2);
        for (wi, (name, accent_name, theme_name, here)) in ws_rows.iter().enumerate() {
            let top = group.y + wi as i32 * rh;
            if wi > 0 {
                canvas.fill(Rect { x: group.x + px(14.0), y: top, w: group.w - px(28.0), h: 1 }, c.edge);
            }
            let cy = top + rh / 2;
            let ctl = |x: i32, w: i32| Rect { x, y: cy - field_h / 2, w, h: field_h };
            let (its, auto) = ws_given.get(wi).copied().unwrap_or((accent, false));
            // Its number on its color.
            let d = px(24.0);
            let badge = Rect { x: group.x + px(14.0), y: cy - d / 2, w: d, h: d };
            canvas.round_fill(badge, d / 2, its, 255);
            let num = format!("{}", wi + 1);
            let nw = fonts.small.width(&num);
            canvas.text(&mut fonts.small, &num, (badge.x + (d - nw) / 2) as f32, cy - sh_ / 2, on(its), badge);
            // Its name, which a click makes a field.
            let nx = badge.x + d + px(10.0);
            let ty = cy - uh / 2;
            let end = match renaming.as_ref().filter(|(rw, _)| *rw == wi) {
                Some((_, field)) => {
                    let b = ctl(nx, px(220.0).min(x_acc - px(16.0) - nx));
                    canvas.round_fill(b, px(7.0), c.field, 255);
                    canvas.ring(b, px(7.0), 1, accent);
                    field.draw(canvas, &mut fonts.ui, b.x + px(8.0), ty, c.text, mix(accent, c.panel, 140), true, b);
                    page.push((b, Hit::WsName(wi)));
                    b.x + b.w
                }
                None => {
                    let room = x_acc - px(90.0) - nx;
                    let b = ctl(nx, (fonts.ui.width(name) + px(16.0)).min(room));
                    if hover == Some(Hit::WsName(wi)) {
                        canvas.round_fill(b, px(7.0), c.hover, 255);
                    }
                    canvas.text_fit(&mut fonts.ui, name, b.x + px(8.0), ty, b.w - px(16.0), c.text, b);
                    page.push((b, Hit::WsName(wi)));
                    b.x + b.w
                }
            };
            if *here {
                let t = "current";
                let pw = fonts.small.width(t) + px(14.0);
                let pill = Rect { x: end + px(6.0), y: cy - (sh_ + px(6.0)) / 2, w: pw, h: sh_ + px(6.0) };
                canvas.round_fill(pill, pill.h / 2, c.hover, 255);
                canvas.text(&mut fonts.small, t, (pill.x + px(7.0)) as f32, pill.y + px(3.0), c.dim, pill);
            }
            if closing == Some(wi) {
                // Asked to close: what it means, and the two ways out.
                let bw = fonts.ui.width("Close") + px(28.0);
                let kw = fonts.ui.width("Keep") + px(28.0);
                let close = ctl(group.x + group.w - gap - bw, bw);
                let keep = ctl(close.x - px(8.0) - kw, kw);
                canvas.round_fill(close, px(7.0), if hover == Some(Hit::WsClose(wi)) { mix(DANGER, 0xFFFFFF, 30) } else { DANGER }, 255);
                canvas.text(&mut fonts.ui, "Close", (close.x + px(14.0)) as f32, ty, 0xFFFFFF, close);
                canvas.round_fill(keep, px(7.0), if hover == Some(Hit::WsKeep) { c.hover } else { c.field }, 255);
                canvas.ring(keep, px(7.0), 1, c.edge);
                canvas.text(&mut fonts.ui, "Keep", (keep.x + px(14.0)) as f32, ty, c.text, keep);
                let ask = "This closes all its panes.";
                let qw = fonts.small.width(ask);
                canvas.text(&mut fonts.small, ask, (keep.x - px(12.0) - qw) as f32, cy - sh_ / 2, c.dim, view);
                page.push((close, Hit::WsClose(wi)));
                page.push((keep, Hit::WsKeep));
                continue;
            }
            // Its accent: the color and what it is, opening the colors.
            let ab = ctl(x_acc, aw);
            canvas.round_fill(ab, px(7.0), if hover == Some(Hit::WsAccentOpen(wi)) { c.hover } else { c.field }, 255);
            canvas.ring(ab, px(7.0), 1, c.edge);
            let sd = px(14.0);
            canvas.round_fill(Rect { x: ab.x + px(10.0), y: cy - sd / 2, w: sd, h: sd }, sd / 2, its, 255);
            let its_name = accent_name.clone().map(|n| if n.starts_with('#') { n } else { capitalize(&n) }).unwrap_or_else(|| if auto { "Auto" } else { "Default" }.into());
            canvas.text_fit(&mut fonts.ui, &its_name, ab.x + px(32.0), ty, aw - px(56.0), if accent_name.is_some() { c.text } else { c.dim }, ab);
            chevron(canvas, ab.x + ab.w - px(16.0), cy, px(4.0), c.dim);
            page.push((ab, Hit::WsAccentOpen(wi)));
            // Its theme.
            let tb = ctl(x_theme, tw);
            canvas.round_fill(tb, px(7.0), if hover == Some(Hit::WsTheme(wi)) { c.hover } else { c.field }, 255);
            canvas.ring(tb, px(7.0), 1, c.edge);
            let shown = theme_name.clone().unwrap_or_else(|| "Default theme".into());
            canvas.text_fit(&mut fonts.ui, &shown, tb.x + px(12.0), ty, tw - px(40.0), if theme_name.is_some() { c.text } else { c.dim }, tb);
            chevron(canvas, tb.x + tb.w - px(16.0), cy, px(4.0), c.dim);
            page.push((tb, Hit::WsTheme(wi)));
            // Go there, and close it: icons.
            if !*here {
                let b = ctl(x_go, ib);
                let lit = hover == Some(Hit::WsGo(wi));
                if lit {
                    canvas.round_fill(b, px(7.0), c.hover, 255);
                }
                arrow(canvas, b.x + ib / 2, cy, px(5.0), dot, if lit { c.text } else { c.dim });
                page.push((b, Hit::WsGo(wi)));
            }
            if many {
                let b = ctl(x_close, ib);
                let lit = hover == Some(Hit::WsClose(wi));
                if lit {
                    canvas.round_fill(b, px(7.0), c.hover, 255);
                }
                cross(canvas, b.x + ib / 2, cy, px(4.0), dot, if lit { DANGER } else { c.dim });
                page.push((b, Hit::WsClose(wi)));
            }
        }
        // A new one, in the focused pane's folder, named as you like.
        let nb = Rect { x: group.x + px(4.0), y: group.y + group.h - rh + px(4.0), w: group.w - px(8.0), h: rh - px(8.0) };
        canvas.fill(Rect { x: group.x + px(14.0), y: group.y + group.h - rh, w: group.w - px(28.0), h: 1 }, c.edge);
        let lit = hover == Some(Hit::WsNew);
        if lit {
            canvas.round_fill(nb, px(7.0), c.hover, 255);
        }
        let pc = (group.x + px(14.0) + px(12.0), nb.y + nb.h / 2);
        let arm = px(6.0);
        canvas.fill(Rect { x: pc.0 - arm, y: pc.1 - dot / 2, w: 2 * arm, h: dot }, accent_text);
        canvas.fill(Rect { x: pc.0 - dot / 2, y: pc.1 - arm, w: dot, h: 2 * arm }, accent_text);
        canvas.text(&mut fonts.ui, "New workspace", (group.x + px(14.0) + px(34.0)) as f32, nb.y + (nb.h - uh) / 2, if lit { c.text } else { accent_text }, nb);
        page.push((nb, Hit::WsNew));
        canvas.ring(group, px(10.0), 1, c.edge);
        y = group.y + group.h + px(24.0);
        }
        if section == 2 {
        // ---- Fonts ----
        heading(canvas, fonts, &mut y, &mut tops, "Fonts");
        let field_w = px(260.0).min(body.w / 2);
        let rows: [(&str, &str, Key); 7] = [
            ("Terminal font", "Used in panes. Monospaced fonts only.", Key::Family),
            ("Terminal font size", "In points. Ctrl = and Ctrl - also change it.", Key::Size),
            ("Line height", "Space between lines, relative to the font. VS Code uses 1.10.", Key::LineHeight),
            ("Ligatures", "Draw pairs like => and != as single symbols.", Key::Ligatures),
            ("Bold text", "Use the font's bold weight. Off keeps all text one weight.", Key::Bold),
            ("Interface font", "Used for menus, captions and settings.", Key::UiFamily),
            ("Interface font size", "In points.", Key::UiSize),
        ];
        for (name, what, key) in rows {
            let mid = title(canvas, fonts, &mut page, y, name, what, key);
            match key {
                Key::Family | Key::UiFamily => {
                    let value = if key == Key::Family { &family } else { &ui_family };
                    let field = Rect { x: right - field_w, y: mid - field_h / 2, w: field_w, h: field_h };
                    canvas.round_fill(field, px(7.0), if hover == Some(Hit::Open(key)) { c.hover } else { c.field }, 255);
                    canvas.ring(field, px(7.0), 1, c.edge);
                    canvas.text_fit(&mut fonts.ui, value, field.x + px(12.0), field.y + (field_h - uh) / 2, field_w - px(44.0), c.text, field);
                    // A chevron at its right.
                    let (vx, vy) = (field.x + field.w - px(18.0), mid + px(2.0));
                    for t in 0..px(5.0) {
                        canvas.fill(Rect { x: vx - t - 1, y: vy - t, w: 2, h: 2 }, c.dim);
                        canvas.fill(Rect { x: vx + t - 1, y: vy - t, w: 2, h: 2 }, c.dim);
                    }
                    page.push((field, Hit::Open(key)));
                }
                Key::Size => stepper(canvas, fonts, &mut page, mid, key, &format!("{size}")),
                Key::LineHeight => stepper(canvas, fonts, &mut page, mid, key, &format!("{line_height:.2}")),
                Key::Ligatures | Key::Bold => {
                    let on = if key == Key::Bold { bold_text } else { ligatures };
                    let (sw, sh) = (px(40.0), px(22.0));
                    let pill = Rect { x: right - sw, y: mid - sh / 2, w: sw, h: sh };
                    canvas.round_fill(pill, sh / 2, if on { accent } else { c.hover }, 255);
                    let k = sh - px(6.0);
                    let kx = if on { pill.x + sw - k - px(3.0) } else { pill.x + px(3.0) };
                    canvas.round_fill(Rect { x: kx, y: pill.y + px(3.0), w: k, h: k }, k / 2, 0xFFFFFF, 255);
                    page.push((Rect { x: pill.x - px(4.0), y: pill.y - px(4.0), w: sw + px(8.0), h: sh + px(8.0) }, Hit::Toggle(key)));
                }
                _ => stepper(canvas, fonts, &mut page, mid, key, &format!("{ui_size}")),
            }
            y += row_h;
            canvas.fill(Rect { x: body.x, y, w: body.w, h: 1 }, rule);
        }
        y += px(32.0);

        }
        if section == 3 {
        // ---- Keys ----
        heading(canvas, fonts, &mut y, &mut tops, "Keys");
        let key_h = sh_ + px(10.0);
        for (keys, what) in KEYS {
            let mut kx = body.x;
            for cap in keys.split("  ") {
                let w = fonts.small.width(cap) + px(14.0);
                let chip = Rect { x: kx, y: y + px(6.0), w, h: key_h };
                canvas.round_fill(chip, px(5.0), c.field, 255);
                canvas.ring(chip, px(5.0), 1, c.edge);
                canvas.text(&mut fonts.small, cap, (chip.x + px(7.0)) as f32, chip.y + px(5.0), c.text, view);
                kx += w + px(6.0);
            }
            canvas.text(&mut fonts.ui, what, (body.x + body.w * 2 / 5) as f32, y + px(6.0) + (key_h - uh) / 2, c.dim, view);
            y += key_h + px(12.0);
        }
        y += px(32.0);

        }
        if section == 4 {
        // ---- About ----
        heading(canvas, fonts, &mut y, &mut tops, "About");
        for (k, v) in [
            ("Version", version),
            ("Terminal", "libghostty-vt 1.0.0".to_string()),
            ("Settings file", persist::shorten(&persist::config_path())),
            ("Shell", std::env::var("SHELL").unwrap_or_default()),
        ] {
            canvas.text(&mut fonts.ui, k, body.x as f32, y, c.dim, view);
            canvas.text(&mut fonts.ui, &v, (body.x + body.w * 2 / 5) as f32, y, c.text, view);
            y += uh + px(14.0);
        }
        }
        // A little room below the last row.
        y += px(24.0);

        st.content_h = y + st.scroll - view.y;
        st.view_h = view.h;
        st.tops = tops;
        st.scroll = st.scroll.clamp(0, (st.content_h - st.view_h).max(0));
        canvas.clip = None;
        // Only what can be seen can be clicked.
        hits.extend(page.into_iter().filter_map(|(rect, h)| {
            let top = rect.y.max(view.y);
            let bottom = (rect.y + rect.h).min(view.y + view.h);
            (bottom > top).then_some((Rect { x: rect.x, y: top, w: rect.w, h: bottom - top }, h))
        }));
        hits.push((close, Hit::Close));

        // The scrollbar, when the page is longer than the view.
        if st.content_h > st.view_h {
            let track = Rect { x: view.x + view.w + px(4.0), y: view.y + px(52.0), w: px(8.0), h: view.h - px(60.0) };
            let thumb_h = (track.h * st.view_h / st.content_h).max(px(32.0));
            let thumb_y = track.y + (track.h - thumb_h) * st.scroll / (st.content_h - st.view_h).max(1);
            let thumb = Rect { x: track.x + px(1.0), y: thumb_y, w: px(6.0), h: thumb_h };
            let busy = st.drag.is_some() || hover == Some(Hit::Thumb);
            canvas.round_fill(thumb, px(3.0), if busy { c.dim } else { c.faint }, 255);
            hits.push((track, Hit::Track));
            hits.push((Rect { x: track.x - px(4.0), y: thumb.y, w: track.w + px(8.0), h: thumb.h }, Hit::Thumb));
        }

        // A workspace's colors, over everything, under its swatch.
        if let Some((wi, anchor)) = st.accent_pop {
            let current = ws_rows.get(wi).and_then(|w| w.1.clone());
            let (d, gap, cols) = (px(22.0), px(10.0), 5);
            let rows = (ACCENTS.len() as i32 + cols - 1) / cols;
            let chip_h = uh + px(12.0);
            let w = cols * (d + gap) - gap + 2 * px(14.0);
            let h = px(12.0) + chip_h + px(10.0) + rows * (d + gap) - gap + px(12.0) + chip_h + px(10.0);
            let below = anchor.y + anchor.h + px(6.0);
            let top = if below + h <= canvas_h { below } else { (anchor.y - h - px(6.0)).max(0) };
            let pop = Rect { x: (anchor.x + anchor.w - w).max(0), y: top, w, h };
            canvas.panel(pop, px(10.0), c.panel, c.edge);
            hits.push((pop, Hit::Track));
            // The settings' own accent first, then the colors.
            let chip = Rect { x: pop.x + px(10.0), y: pop.y + px(10.0), w: pop.w - px(20.0), h: chip_h };
            let on_settings = current.is_none();
            canvas.round_fill(chip, px(6.0), if on_settings { c.hover } else if hover == Some(Hit::WsAccent(wi, usize::MAX)) { c.field } else { c.panel }, 255);
            let dot = Rect { x: chip.x + px(8.0), y: chip.y + (chip_h - px(12.0)) / 2, w: px(12.0), h: px(12.0) };
            if ws_colors && wi > 0 {
                canvas.ring(dot, px(6.0), px(1.5).max(1), c.dim);
            } else {
                canvas.round_fill(dot, px(6.0), self.accent, 255);
            }
            canvas.text(&mut fonts.ui, if ws_colors && wi > 0 { "Auto" } else { "Default accent" }, (chip.x + px(28.0)) as f32, chip.y + (chip_h - uh) / 2, c.text, chip);
            hits.push((chip, Hit::WsAccent(wi, usize::MAX)));
            for (i, (an, color)) in ACCENTS.iter().enumerate() {
                let (col, row) = (i as i32 % cols, i as i32 / cols);
                let dot = Rect { x: pop.x + px(14.0) + col * (d + gap), y: chip.y + chip_h + px(10.0) + row * (d + gap), w: d, h: d };
                canvas.round_fill(dot, d / 2, *color, 255);
                if current.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(an)) {
                    canvas.ring(Rect { x: dot.x - px(3.0), y: dot.y - px(3.0), w: d + px(6.0), h: d + px(6.0) }, d, px(2.0), c.text);
                } else if hover == Some(Hit::WsAccent(wi, i)) {
                    canvas.ring(Rect { x: dot.x - px(2.0), y: dot.y - px(2.0), w: d + px(4.0), h: d + px(4.0) }, d, 1, c.dim);
                }
                hits.push((dot, Hit::WsAccent(wi, i)));
            }
            // Any color, typed as hex; Enter takes it.
            let hb = Rect { x: pop.x + px(10.0), y: chip.y + chip_h + px(10.0) + rows * (d + gap) - gap + px(12.0), w: pop.w - px(20.0), h: chip_h };
            let typed = st.hex.string();
            let valid = hex_color(&typed);
            canvas.round_fill(hb, px(6.0), c.field, 255);
            canvas.ring(hb, px(6.0), 1, if valid.is_some() { accent } else { c.edge });
            let ty = hb.y + (chip_h - uh) / 2;
            let tx = canvas.text(&mut fonts.ui, "#", (hb.x + px(10.0)) as f32, ty, c.faint, hb) + px(2.0);
            if typed.is_empty() {
                canvas.text(&mut fonts.ui, "ff8800", tx as f32, ty, c.faint, hb);
            }
            st.hex.draw(canvas, &mut fonts.ui, tx, ty, c.text, mix(accent, c.panel, 140), true, hb);
            if let Some(color) = valid {
                let sd = px(14.0);
                canvas.round_fill(Rect { x: hb.x + hb.w - px(10.0) - sd, y: hb.y + (chip_h - sd) / 2, w: sd, h: sd }, sd / 2, color, 255);
            }
        }

        // A font list, over everything, under its field.
        if let Some(d) = &mut st.dropdown {
            let row = uh + px(12.0);
            let rows = 10usize;
            let w = d.anchor.w.max(px(280.0));
            let h = px(8.0) + field_h + px(6.0) + rows as i32 * row + px(8.0);
            let below = d.anchor.y + d.anchor.h + px(4.0);
            let top = if below + h <= canvas_h { below } else { (d.anchor.y - h - px(4.0)).max(0) };
            let list = Rect { x: (d.anchor.x + d.anchor.w - w).max(0), y: top, w, h };
            canvas.panel(list, px(10.0), c.panel, c.edge);
            let filter = Rect { x: list.x + px(8.0), y: list.y + px(8.0), w: list.w - px(16.0), h: field_h };
            canvas.round_fill(filter, px(6.0), c.field, 255);
            let ty = filter.y + (field_h - uh) / 2;
            if d.filter.text.is_empty() {
                canvas.text(&mut fonts.ui, "Type to filter", (filter.x + px(10.0)) as f32, ty, c.faint, filter);
                canvas.fill(Rect { x: filter.x + px(10.0), y: ty, w: px(2.0).max(2), h: uh }, c.text);
            } else {
                d.filter.draw(canvas, &mut fonts.ui, filter.x + px(10.0), ty, c.text, mix(accent, c.panel, 140), true, filter);
            }
            let shown = d.shown();
            d.selected = d.selected.min(shown.len().saturating_sub(1));
            if d.selected < d.first {
                d.first = d.selected;
            } else if d.selected >= d.first + rows {
                d.first = d.selected + 1 - rows;
            }
            d.first = d.first.min(shown.len().saturating_sub(rows));
            hits.push((list, Hit::Track));
            let ws_theme_name = if d.key == Key::WsTheme { ws_rows.get(d.ws).map(|w| w.2.clone().unwrap_or_else(|| "Default theme".into())).unwrap_or_default() } else { String::new() };
            let current = match d.key {
                Key::Family => &family,
                Key::WsTheme => &ws_theme_name,
                _ => &ui_family,
            };
            for (k, (i, name)) in shown.iter().enumerate().skip(d.first).take(rows) {
                let rr = Rect { x: list.x + px(6.0), y: filter.y + field_h + px(6.0) + (k - d.first) as i32 * row, w: list.w - px(12.0), h: row };
                if k == d.selected {
                    canvas.round_fill(rr, px(6.0), accent, 255);
                } else if hover == Some(Hit::Item(*i)) {
                    canvas.round_fill(rr, px(6.0), c.field, 255);
                }
                let fg = if k == d.selected { on(accent) } else if name.eq_ignore_ascii_case(current) { accent_text } else { c.text };
                canvas.text_fit(&mut fonts.ui, name, rr.x + px(10.0), rr.y + (row - uh) / 2, rr.w - px(20.0), fg, rr);
                hits.push((rr, Hit::Item(*i)));
            }
            if shown.is_empty() {
                canvas.text(&mut fonts.ui, "No font by that name", (list.x + px(16.0)) as f32, filter.y + field_h + px(14.0), c.faint, list);
            }
        }

        st.hits = hits;
        self.settings = Some(st);
        self.damage.push(Rect { x: 0, y: 0, w: self.canvas.width, h: self.canvas.height });
    }
}

/// A chevron pointing down, as on a list that opens.
fn chevron(canvas: &mut Canvas, cx: i32, cy: i32, size: i32, color: u32) {
    let y = cy + size / 2;
    for t in 0..size {
        canvas.fill(Rect { x: cx - t - 1, y: y - t, w: 2, h: 2 }, color);
        canvas.fill(Rect { x: cx + t - 1, y: y - t, w: 2, h: 2 }, color);
    }
}

/// An arrow pointing right, for going somewhere.
fn arrow(canvas: &mut Canvas, cx: i32, cy: i32, arm: i32, dot: i32, color: u32) {
    canvas.fill(Rect { x: cx - arm, y: cy - dot / 2, w: 2 * arm, h: dot }, color);
    for t in 0..=arm * 3 / 5 {
        canvas.round_fill(Rect { x: cx + arm - t - dot / 2, y: cy - t - dot / 2, w: dot, h: dot }, dot / 2, color, 255);
        canvas.round_fill(Rect { x: cx + arm - t - dot / 2, y: cy + t - dot / 2, w: dot, h: dot }, dot / 2, color, 255);
    }
}

/// An ×, for closing.
fn cross(canvas: &mut Canvas, cx: i32, cy: i32, arm: i32, dot: i32, color: u32) {
    for t in -arm..=arm {
        canvas.round_fill(Rect { x: cx + t - dot / 2, y: cy + t - dot / 2, w: dot, h: dot }, dot / 2, color, 255);
        canvas.round_fill(Rect { x: cx + t - dot / 2, y: cy - t - dot / 2, w: dot, h: dot }, dot / 2, color, 255);
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// A color typed as RRGGBB (or RGB), with or without its #.
fn hex_color(s: &str) -> Option<u32> {
    let s = s.trim().trim_start_matches('#');
    let s: String = match s.len() {
        3 => s.chars().flat_map(|c| [c, c]).collect(),
        6 => s.to_string(),
        _ => return None,
    };
    u32::from_str_radix(&s, 16).ok()
}
