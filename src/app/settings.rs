//! Settings, on Ctrl , or from the right-click menu and the palette: a
//! list of sections down the left, the chosen one's settings on the
//! right. What is changed here is seen at once and written to the config.

use super::*;
use crate::theme::THEMES;

const PANEL: u32 = 0x1C1D21;
const SIDE: u32 = 0x18191C;
const EDGE: u32 = 0x3A3C43;
const TEXT: u32 = 0xEEEEEE;
const DIM: u32 = 0x8E9199;
const ACCENT: u32 = 0x3E63DD;
const HOVER: u32 = 0x2A2C33;

const SECTIONS: [&str; 3] = ["Appearance", "Keys", "About"];

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hit {
    Section(usize),
    Theme(usize),
    Smaller,
    Larger,
    StatusLine,
    Captions,
    Close,
}

pub struct Settings {
    pub section: usize,
    /// The theme picked out with the keys.
    pub picked: usize,
    /// The first theme shown, when not all fit.
    pub scroll: usize,
    pub hover: Option<Hit>,
    /// What can be clicked, where it was last drawn.
    pub hits: Vec<(Rect, Hit)>,
}

const KEYS: &[(&str, &str)] = &[
    ("Ctrl K", "Palette: workspaces, commands, projects"),
    ("Ctrl ,", "Settings"),
    ("Alt V  /  Alt S", "Split side by side / one above another"),
    ("Alt H J K L", "Move to the pane left, down, up, right"),
    ("Alt X", "Close the pane"),
    ("Alt Shift M", "The pane alone, and back"),
    ("Alt arrows  /  Alt =", "Resize / make all equal"),
    ("Ctrl Q", "Close every pane but this one"),
    ("Alt 1–9  /  Alt , .", "Go to a workspace / the one before, after"),
    ("Alt Shift N", "New workspace in this pane's folder"),
    ("Alt `", "Roaming terminal"),
    ("Ctrl Shift C  /  V", "Copy / paste"),
    ("Shift PageUp / PageDown", "Scroll back"),
    ("Ctrl =  -  0", "Text larger, smaller, as it was"),
];

impl App {
    pub(super) fn open_settings(&mut self) {
        self.palette = None;
        self.menu = None;
        let picked = THEMES.iter().position(|t| std::ptr::eq(t, self.theme)).unwrap_or(0);
        self.settings = Some(Settings { section: 0, picked, scroll: 0, hover: None, hits: Vec::new() });
        self.full = true;
        self.redraw();
    }

    pub(super) fn set_theme(&mut self, i: usize) {
        let Some(theme) = THEMES.get(i) else { return };
        self.theme = theme;
        for p in self.panes.values_mut() {
            p.term.set_theme(theme);
            p.full = true;
        }
        persist::set_config_value("theme", &format!("{:?}", theme.name));
        self.full = true;
        self.redraw();
    }

    fn set_flag(&mut self, hit: Hit) {
        let (key, on) = match hit {
            Hit::StatusLine => {
                self.config.status_line = !self.config.status_line;
                ("status-line", self.config.status_line)
            }
            _ => {
                self.config.captions = !self.config.captions;
                ("captions", self.config.captions)
            }
        };
        persist::set_config_value(key, if on { "true" } else { "false" });
        self.relayout();
    }

    fn settings_do(&mut self, hit: Hit) {
        match hit {
            Hit::Section(i) => {
                if let Some(s) = &mut self.settings {
                    s.section = i;
                }
            }
            Hit::Theme(i) => {
                if let Some(s) = &mut self.settings {
                    s.picked = i;
                }
                self.set_theme(i);
            }
            Hit::Smaller | Hit::Larger => {
                let size = self.font_size + if hit == Hit::Larger { 1.0 } else { -1.0 };
                self.set_font_size(size);
                self.config.font_size = self.font_size;
                persist::set_config_value("font-size", &format!("{}", self.font_size));
            }
            Hit::StatusLine | Hit::Captions => self.set_flag(hit),
            Hit::Close => self.settings = None,
        }
        self.full = true;
        self.redraw();
    }

    /// Rows of themes that fit, from the last drawing.
    fn theme_rows(&self) -> usize {
        self.settings.as_ref().map_or(8, |s| s.hits.iter().filter(|(_, h)| matches!(h, Hit::Theme(_))).count().max(1))
    }

    fn show_picked(&mut self) {
        let rows = self.theme_rows();
        if let Some(s) = &mut self.settings {
            if s.picked < s.scroll {
                s.scroll = s.picked;
            } else if s.picked >= s.scroll + rows {
                s.scroll = s.picked + 1 - rows;
            }
        }
    }

    pub(super) fn settings_key(&mut self, event: &winit::event::KeyEvent) {
        let PhysicalKey::Code(code) = event.physical_key else { return };
        let Some(s) = &mut self.settings else { return };
        match code {
            KeyCode::Escape => self.settings = None,
            KeyCode::Tab if self.mods.shift_key() => s.section = (s.section + SECTIONS.len() - 1) % SECTIONS.len(),
            KeyCode::Tab => s.section = (s.section + 1) % SECTIONS.len(),
            KeyCode::ArrowUp | KeyCode::ArrowDown if s.section == 0 => {
                s.picked = if code == KeyCode::ArrowUp { s.picked.saturating_sub(1) } else { (s.picked + 1).min(THEMES.len() - 1) };
                let i = s.picked;
                self.show_picked();
                // A theme is tried as soon as it is picked out.
                self.set_theme(i);
            }
            KeyCode::Minus => return self.settings_do(Hit::Smaller),
            KeyCode::Equal => return self.settings_do(Hit::Larger),
            _ => return,
        }
        self.full = true;
        self.redraw();
    }

    fn settings_rect(&self) -> Rect {
        let (w, h) = (self.canvas.width, self.canvas.height);
        let width = (self.px(14.0) * 58).min(w - 2 * self.px(16.0));
        let height = (self.px(14.0) * 38).min(h - 2 * self.px(16.0));
        Rect { x: (w - width) / 2, y: (h - height) / 2, w: width, h: height }
    }

    pub(super) fn settings_hit(&self, x: i32, y: i32) -> Option<Hit> {
        self.settings.as_ref()?.hits.iter().rev().find(|(r, _)| r.contains(x, y)).map(|(_, h)| *h)
    }

    /// A click while the settings are open: theirs, whatever it is on. A
    /// click outside closes them.
    pub(super) fn settings_click(&mut self) -> bool {
        if self.settings.is_none() {
            return false;
        }
        let (x, y) = self.pointer;
        match self.settings_hit(x, y) {
            Some(hit) => self.settings_do(hit),
            None if !self.settings_rect().contains(x, y) => {
                self.settings = None;
                self.full = true;
                self.redraw();
            }
            None => {}
        }
        true
    }

    pub(super) fn settings_hover(&mut self, x: i32, y: i32) -> bool {
        if self.settings.is_none() {
            return false;
        }
        let hit = self.settings_hit(x, y);
        if let Some(s) = &mut self.settings {
            if s.hover != hit {
                s.hover = hit;
                self.full = true;
                self.redraw();
            }
        }
        true
    }

    pub(super) fn settings_scroll(&mut self, lines: isize) -> bool {
        let rows = self.theme_rows();
        let Some(s) = &mut self.settings else { return false };
        if s.section == 0 {
            let max = THEMES.len().saturating_sub(rows);
            s.scroll = (s.scroll as isize + lines.signum()).clamp(0, max as isize) as usize;
            self.full = true;
            self.redraw();
        }
        true
    }

    pub(super) fn draw_settings(&mut self) {
        let scale = self.scale;
        let px = |v: f32| (v * scale).round() as i32;
        let Some(s) = self.settings.as_ref() else { return };
        let (section, picked, scroll, hover) = (s.section, s.picked, s.scroll, s.hover);
        let r = self.settings_rect();
        let pad = px(16.0);
        let line = self.fonts.ui.height;
        let row_h = line * 2 + px(2.0);
        let mut hits = Vec::new();
        // The panes behind are dimmed.
        self.canvas.round_fill(Rect { x: 0, y: 0, w: self.canvas.width, h: self.canvas.height }, 0, 0x000000, 110);
        self.canvas.panel(r, px(12.0), PANEL, EDGE);

        // Down the left: the sections.
        let side_w = (r.w * 3 / 10).min(px(220.0));
        let side = Rect { x: r.x + 1, y: r.y + 1, w: side_w, h: r.h - 2 };
        self.canvas.round_fill(side, px(11.0), SIDE, 255);
        self.canvas.fill(Rect { x: side.x + side.w - px(12.0), y: side.y, w: px(12.0), h: side.h }, SIDE);
        self.canvas.fill(Rect { x: side.x + side.w, y: side.y, w: 1, h: side.h }, EDGE);
        let ui = &mut self.fonts.ui;
        self.canvas.text(ui, "Settings", (side.x + pad) as f32, side.y + pad, DIM, side);
        let mut top = side.y + pad + line + pad;
        for (i, name) in SECTIONS.iter().enumerate() {
            let row = Rect { x: side.x + pad / 2, y: top, w: side.w - pad, h: row_h };
            let on = i == section;
            if on {
                self.canvas.round_fill(row, px(6.0), ACCENT, 255);
            } else if hover == Some(Hit::Section(i)) {
                self.canvas.round_fill(row, px(6.0), HOVER, 255);
            }
            self.canvas.text(ui, name, (row.x + pad / 2) as f32, row.y + (row_h - line) / 2, if on { 0xFFFFFF } else { TEXT }, row);
            hits.push((row, Hit::Section(i)));
            top += row_h + px(2.0);
        }

        // On the right: the section's settings.
        let body = Rect { x: side.x + side.w + pad * 3 / 2, y: r.y + pad, w: r.x + r.w - (side.x + side.w) - 3 * pad, h: r.h - 2 * pad };
        let esc_w = ui.width("Esc") + px(12.0);
        let close = Rect { x: r.x + r.w - pad - esc_w, y: r.y + pad - px(4.0), w: esc_w, h: line + px(8.0) };
        if hover == Some(Hit::Close) {
            self.canvas.round_fill(close, px(6.0), HOVER, 255);
        }
        self.canvas.text(ui, "Esc", (close.x + px(6.0)) as f32, close.y + px(4.0), DIM, close);
        hits.push((close, Hit::Close));
        self.canvas.text(ui, SECTIONS[section], body.x as f32, body.y, TEXT, body);
        let mut y = body.y + line + pad;
        match section {
            0 => {
                // Text size, then the two switches, then the themes.
                let label = |c: &mut Canvas, ui: &mut crate::font::UiFont, t: &str, y: i32| {
                    c.text(ui, t, body.x as f32, y + (row_h - line) / 2, TEXT, body);
                };
                label(&mut self.canvas, ui, "Text size", y);
                let b = row_h - px(8.0);
                let right = body.x + body.w;
                let plus = Rect { x: right - b, y: y + px(4.0), w: b, h: b };
                let size = format!("{}", self.font_size);
                let size_w = ui.width(&size) + pad;
                let minus = Rect { x: plus.x - size_w - b, y: plus.y, w: b, h: b };
                for (rect, hit, glyph) in [(minus, Hit::Smaller, "−"), (plus, Hit::Larger, "+")] {
                    self.canvas.round_fill(rect, px(6.0), if hover == Some(hit) { EDGE } else { HOVER }, 255);
                    let gw = ui.width(glyph);
                    self.canvas.text(ui, glyph, (rect.x + (b - gw) / 2) as f32, rect.y + (b - line) / 2, TEXT, rect);
                    hits.push((rect, hit));
                }
                self.canvas.text(ui, &size, (minus.x + b + pad / 2) as f32, y + (row_h - line) / 2, TEXT, body);
                y += row_h;
                for (name, hit, on) in [("Captions under panes", Hit::Captions, self.config.captions), ("Status line along the bottom", Hit::StatusLine, self.config.status_line)] {
                    let row = Rect { x: body.x - pad / 2, y, w: body.w + pad, h: row_h };
                    if hover == Some(hit) {
                        self.canvas.round_fill(row, px(6.0), HOVER, 255);
                    }
                    label(&mut self.canvas, ui, name, y);
                    // A switch: a pill, its knob to the right when on.
                    let (sw, sh) = (px(34.0), px(20.0));
                    let pill = Rect { x: right - sw, y: y + (row_h - sh) / 2, w: sw, h: sh };
                    self.canvas.round_fill(pill, sh / 2, if on { ACCENT } else { EDGE }, 255);
                    let k = sh - px(6.0);
                    let kx = if on { pill.x + sw - k - px(3.0) } else { pill.x + px(3.0) };
                    self.canvas.round_fill(Rect { x: kx, y: pill.y + px(3.0), w: k, h: k }, k / 2, 0xFFFFFF, 255);
                    hits.push((row, hit));
                    y += row_h;
                }
                y += pad / 2;
                self.canvas.text(ui, "Theme", body.x as f32, y, DIM, body);
                y += line + pad / 2;
                let rows = ((body.y + body.h - y) / row_h).max(1) as usize;
                let current = THEMES.iter().position(|t| std::ptr::eq(t, self.theme));
                for (i, theme) in THEMES.iter().enumerate().skip(scroll).take(rows) {
                    let row = Rect { x: body.x - pad / 2, y, w: body.w + pad, h: row_h - px(2.0) };
                    if Some(i) == current {
                        self.canvas.round_fill(row, px(6.0), ACCENT, 255);
                    } else if hover == Some(Hit::Theme(i)) || i == picked {
                        self.canvas.round_fill(row, px(6.0), HOVER, 255);
                    }
                    // A swatch: the theme's ground with its colours on it.
                    let dot = px(8.0);
                    let sw = Rect { x: row.x + pad / 2, y: row.y + px(6.0), w: dot * 9 + px(10.0), h: row.h - px(12.0) };
                    self.canvas.round_fill(sw, px(4.0), theme.bg, 255);
                    for (k, c) in [theme.fg, theme.ansi[1], theme.ansi[2], theme.ansi[3], theme.ansi[4], theme.ansi[5], theme.ansi[6]].iter().enumerate() {
                        let d = Rect { x: sw.x + px(6.0) + k as i32 * (dot + px(3.0)), y: sw.y + (sw.h - dot) / 2, w: dot, h: dot };
                        self.canvas.round_fill(d, dot / 2, *c, 255);
                    }
                    let fg = if Some(i) == current { 0xFFFFFF } else { TEXT };
                    self.canvas.text(ui, theme.name, (sw.x + sw.w + pad) as f32, row.y + (row.h - line) / 2, fg, row);
                    let kind = if theme.dark { "dark" } else { "light" };
                    let kw = ui.width(kind);
                    self.canvas.text(ui, kind, (row.x + row.w - pad / 2 - kw) as f32, row.y + (row.h - line) / 2, if Some(i) == current { 0xC5D0F5 } else { DIM }, row);
                    hits.push((row, Hit::Theme(i)));
                    y += row_h;
                }
            }
            1 => {
                for (keys, what) in KEYS {
                    if y + line > body.y + body.h {
                        break;
                    }
                    self.canvas.text(ui, keys, body.x as f32, y, TEXT, body);
                    self.canvas.text(ui, what, (body.x + body.w * 4 / 10) as f32, y, DIM, body);
                    y += line + px(10.0);
                }
            }
            _ => {
                let config = persist::config_path();
                let lines = [
                    ("Version", crate::version()),
                    ("Terminal", "libghostty-vt 1.0.0".to_string()),
                    ("Settings file", persist::shorten(&config)),
                    ("Shell", std::env::var("SHELL").unwrap_or_default()),
                ];
                for (k, v) in lines {
                    self.canvas.text(ui, k, body.x as f32, y, DIM, body);
                    self.canvas.text(ui, &v, (body.x + body.w * 3 / 10) as f32, y, TEXT, body);
                    y += line + px(12.0);
                }
            }
        }
        if let Some(s) = &mut self.settings {
            s.hits = hits;
        }
        self.damage.push(Rect { x: 0, y: 0, w: self.canvas.width, h: self.canvas.height });
    }
}
