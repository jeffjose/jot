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
}

const ALL_KEYS: [Key; 10] = [Key::Theme, Key::Accent, Key::Contrast, Key::Family, Key::Size, Key::LineHeight, Key::UiFamily, Key::UiSize, Key::Captions, Key::StatusLine];

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
    WsBroadcast(usize),
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
    /// The scrollbar's thumb being dragged: where from, and the scroll then.
    pub drag: Option<(i32, i32)>,
    /// The installed fonts, read once: monospaced, and all.
    mono: Option<Vec<String>>,
    all: Option<Vec<String>>,
}

const KEYS: &[(&str, &str)] = &[
    ("Ctrl F", "Find in the pane; Enter for the match above"),
    ("Ctrl K", "Find a workspace, pane or project folder"),
    ("Ctrl P", "Run a command"),
    ("Ctrl ,", "Settings"),
    ("Alt V", "Split side by side"),
    ("Alt S", "Split one above another"),
    ("Alt H J K L", "Move to the pane left, down, up, right"),
    ("Ctrl T", "New pane to the right of this one"),
    ("Ctrl W  Alt X", "Close the pane"),
    ("Ctrl Shift T", "Bring back the pane closed last"),
    ("Alt Shift M", "The pane alone, and back"),
    ("Alt Arrows", "Resize the pane"),
    ("Alt =", "Make all panes equal"),
    ("Alt 1–9", "Go to a workspace"),
    ("Alt ,  Alt .", "The workspace before, after"),
    ("Alt Shift N", "New workspace in this pane's folder"),
    ("Alt Shift B", "Type into every pane of the workspace, and back"),
    ("Alt `", "Roaming terminal"),
    ("Ctrl Shift C", "Copy"),
    ("Ctrl Shift V", "Paste"),
    ("Shift PageUp", "Scroll back"),
    ("Ctrl =  -  0", "Text larger, smaller, as it was"),
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
    }
}

impl App {
    pub(super) fn open_settings(&mut self) {
        self.palette = None;
        self.menu = None;
        self.settings = Some(Settings { scroll: 0, hover: None, hits: Vec::new(), content_h: 0, view_h: 1, tops: Vec::new(), section: 0, dropdown: None, drag: None, mono: None, all: None });
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
            Key::StatusLine => {
                self.config.status_line = false;
                self.relayout();
            }
        }
        persist::unset_config_value(config_key(key));
    }

    fn settings_do(&mut self, hit: Hit) {
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
                let on = if key == Key::Captions {
                    self.config.captions = !self.config.captions;
                    self.config.captions
                } else {
                    self.config.status_line = !self.config.status_line;
                    self.config.status_line
                };
                persist::set_config_value(config_key(key), if on { "true" } else { "false" });
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
            Hit::WsAccent(wi, i) => {
                if let Some(ws) = self.workspaces.get_mut(wi) {
                    ws.accent = ACCENTS.get(i).map(|(n, _)| n.to_string());
                }
                self.unsaved = true;
            }
            Hit::WsBroadcast(wi) => {
                if let Some(ws) = self.workspaces.get_mut(wi) {
                    ws.broadcast = !ws.broadcast;
                }
                self.refresh_panes();
            }
            Hit::WsTheme(wi) => {
                let anchor = self.settings.as_ref().and_then(|s| s.hits.iter().find(|(_, h)| *h == hit).map(|(r, _)| *r)).unwrap_or_default();
                let current = self.workspaces.get(wi).and_then(|w| w.theme.clone());
                let mut items = vec!["Settings' theme".to_string()];
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
    pub(super) fn settings_click(&mut self) -> bool {
        if self.settings.is_none() {
            return false;
        }
        let (x, y) = self.pointer;
        let hit = self.settings_hit(x, y);
        let list_open = self.settings.as_ref().is_some_and(|s| s.dropdown.is_some());
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
        let c = self.cur_theme.chrome();
        let Some(mut st) = self.settings.take() else { return };
        let scale = self.scale;
        let px = |v: f32| (v * scale).round() as i32;
        let r = self.settings_rect();
        let accent = self.cur_accent;
        let accent_text = mix(accent, 0xFFFFFF, 110);
        let mut hits: Vec<(Rect, Hit)> = Vec::new();
        let hover = st.hover;
        let modified: Vec<Key> = ALL_KEYS.into_iter().filter(|k| self.modified(*k)).collect();
        let current_theme = THEMES.iter().position(|t| std::ptr::eq(t, self.theme));
        let current_accent = ACCENTS.iter().position(|(_, c)| *c == self.accent);
        let (family, size, ui_family, ui_size, line_height) = (self.config.font_family.clone(), self.font_size, self.config.ui_font_family.clone(), self.config.ui_font_size, self.config.line_height);
        let (captions, status_line) = (self.config.captions, self.config.status_line);
        let ws_rows: Vec<(String, Option<String>, Option<String>, bool, bool)> = self.workspaces.iter().enumerate().map(|(i, w)| (w.name.clone(), w.accent.clone(), w.theme.clone(), w.broadcast, i == self.current)).collect();
        let min_contrast = self.config.min_contrast;
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
        let field_w = px(260.0).min(body.w / 2);
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
        title(canvas, fonts, &mut page, y, "Color theme", "The terminal's colours, and the panes' around them.", Key::Theme);
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
                // A few lines of code in the theme's colours.
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

        title(canvas, fonts, &mut page, y, "Accent", "What is picked out: the pane you are in, menus, switches.", Key::Accent);
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
            let mid = title(canvas, fonts, &mut page, y, "Minimum contrast", "Text is made darker or lighter until it can be read on its ground, as in VS Code (4.5). 1 is off.", Key::Contrast);
            stepper(canvas, fonts, &mut page, mid, Key::Contrast, &format!("{min_contrast:.1}"));
            y += row_h;
            canvas.fill(Rect { x: body.x, y, w: body.w, h: 1 }, rule);
        }
        for (name, what, key, value) in [
            ("Captions under panes", "Each pane's title along its bottom, as screen's regions have.", Key::Captions, captions),
            ("Status line", "Workspaces, host and clock along the bottom of the window.", Key::StatusLine, status_line),
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
        canvas.text(&mut fonts.small, "Each workspace can have its own accent and theme, over the ones under Appearance.", body.x as f32, y - px(8.0), c.dim, view);
        y += sh_ + px(12.0);
        for (wi, (name, accent_name, theme_name, broadcast, here)) in ws_rows.iter().enumerate() {
            // The workspace's name, its accent as a dot.
            let its = accent_name.as_deref().map(crate::theme::accent).unwrap_or(accent);
            canvas.round_fill(Rect { x: body.x, y: y + px(4.0) + (uh - px(10.0)) / 2, w: px(10.0), h: px(10.0) }, px(5.0), its, 255);
            let end = canvas.text(&mut fonts.ui, name, (body.x + px(18.0)) as f32, y + px(4.0), c.text, view);
            if *here {
                canvas.text(&mut fonts.small, "you are here", (end + px(10.0)) as f32, y + px(4.0) + (uh - sh_) / 2, c.faint, view);
            }
            // Its theme, at the right.
            let field = Rect { x: right - field_w, y, w: field_w, h: field_h };
            canvas.round_fill(field, px(7.0), if hover == Some(Hit::WsTheme(wi)) { c.hover } else { c.field }, 255);
            canvas.ring(field, px(7.0), 1, c.edge);
            let shown = theme_name.clone().unwrap_or_else(|| "Settings' theme".into());
            canvas.text_fit(&mut fonts.ui, &shown, field.x + px(12.0), field.y + (field_h - uh) / 2, field_w - px(44.0), if theme_name.is_some() { c.text } else { c.dim }, field);
            page.push((field, Hit::WsTheme(wi)));
            y += field_h + px(10.0);
            // Its accent: the settings', or a colour of its own.
            let d = px(20.0);
            let gap = px(8.0);
            let per_row = ((body.w - px(90.0) + gap) / (d + gap)).max(1);
            let chip = Rect { x: body.x, y, w: px(78.0), h: d };
            canvas.round_fill(chip, d / 2, if accent_name.is_none() { c.hover } else { c.field }, 255);
            if accent_name.is_none() {
                canvas.ring(chip, d / 2, 1, c.text);
            }
            canvas.text(&mut fonts.small, "Settings'", (chip.x + px(10.0)) as f32, chip.y + (d - sh_) / 2, c.text, view);
            page.push((chip, Hit::WsAccent(wi, usize::MAX)));
            for (i, (an, color)) in ACCENTS.iter().enumerate() {
                let (col, rowi) = (i as i32 % per_row, i as i32 / per_row);
                let dot = Rect { x: body.x + px(90.0) + col * (d + gap), y: y + rowi * (d + gap), w: d, h: d };
                canvas.round_fill(dot, d / 2, *color, 255);
                if accent_name.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(an)) {
                    canvas.ring(Rect { x: dot.x - px(3.0), y: dot.y - px(3.0), w: d + px(6.0), h: d + px(6.0) }, d, px(2.0), c.text);
                }
                page.push((dot, Hit::WsAccent(wi, i)));
            }
            y += ((ACCENTS.len() as i32 + per_row - 1) / per_row) * (d + gap) + px(6.0);
            // Typing into all its panes.
            let mid = y + row_h / 2 - row_pad;
            canvas.text(&mut fonts.ui, "Type into every pane", body.x as f32, mid - uh / 2, c.text, view);
            canvas.text(&mut fonts.small, "Alt Shift B", (body.x + px(170.0)) as f32, mid - sh_ / 2, c.faint, view);
            let (sw, shh) = (px(40.0), px(22.0));
            let pill = Rect { x: right - sw, y: mid - shh / 2, w: sw, h: shh };
            canvas.round_fill(pill, shh / 2, if *broadcast { accent } else { c.hover }, 255);
            let k = shh - px(6.0);
            let kx = if *broadcast { pill.x + sw - k - px(3.0) } else { pill.x + px(3.0) };
            canvas.round_fill(Rect { x: kx, y: pill.y + px(3.0), w: k, h: k }, k / 2, 0xFFFFFF, 255);
            page.push((Rect { x: pill.x - px(4.0), y: pill.y - px(4.0), w: sw + px(8.0), h: shh + px(8.0) }, Hit::WsBroadcast(wi)));
            y += row_h - row_pad;
            canvas.fill(Rect { x: body.x, y, w: body.w, h: 1 }, rule);
            y += px(16.0);
        }
        }
        if section == 2 {
        // ---- Fonts ----
        heading(canvas, fonts, &mut y, &mut tops, "Fonts");
        let field_w = px(260.0).min(body.w / 2);
        let rows: [(&str, &str, Key); 5] = [
            ("Terminal font", "For the panes. Only fonts whose letters are all one width.", Key::Family),
            ("Terminal font size", "In points. Ctrl = and Ctrl - change it too.", Key::Size),
            ("Line height", "Room between lines, as a share of the font's own. VS Code's is 1.10.", Key::LineHeight),
            ("Interface font", "For jterm's own words: captions, menus, these settings.", Key::UiFamily),
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
            let ws_theme_name = if d.key == Key::WsTheme { ws_rows.get(d.ws).map(|w| w.2.clone().unwrap_or_else(|| "Settings' theme".into())).unwrap_or_default() } else { String::new() };
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
