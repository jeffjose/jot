//! Find, on Ctrl F, as VS Code's terminal has it: a small box at the top
//! right of the pane, the matches lit in the pane, Enter for the one
//! above (older), Shift Enter for the one below, Esc to close.

use super::*;
use crate::canvas::{mix, on};
use crate::field::{Outcome, TextField};
use crate::vt::Span;

/// The amber of the matches, and the orange of the one picked.
const MATCH: u32 = 0xFFC53D;
const PICKED: u32 = 0xF76B15;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FindHit {
    Up,
    Down,
    Close,
    Field,
}

pub struct Find {
    pub pane: PaneId,
    pub field: TextField,
    pub search: vt::Search,
    pub hover: Option<FindHit>,
    pub hits: Vec<(Rect, FindHit)>,
}

impl App {
    pub(super) fn open_find(&mut self) {
        let Some(id) = self.focused() else { return };
        // Open already on this pane: what is typed is selected, to type over.
        if let Some(f) = self.find.as_mut().filter(|f| f.pane == id) {
            f.field.anchor = Some(0);
            f.field.cursor = f.field.text.len();
            self.full = true;
            return self.redraw();
        }
        self.close_find();
        let Some(p) = self.panes.get(&id) else { return };
        // A selection of one line is what to find.
        let start = p.term.selection_text().filter(|t| !t.contains('\n')).unwrap_or_default();
        let mut field = TextField::new(&start);
        field.anchor = Some(0);
        let mut search = p.term.search();
        if !start.is_empty() {
            search.set(&start);
            search.step(true);
        }
        self.find = Some(Find { pane: id, field, search, hover: None, hits: Vec::new() });
        self.refresh_find();
    }

    pub(super) fn close_find(&mut self) {
        if let Some(f) = self.find.take() {
            if let Some(p) = self.panes.get_mut(&f.pane) {
                p.full = true;
            }
            self.full = true;
            self.redraw();
        }
    }

    fn refresh_find(&mut self) {
        if let Some(p) = self.find.as_ref().and_then(|f| self.panes.get_mut(&f.pane)) {
            p.full = true;
        }
        self.full = true;
        self.redraw();
    }

    fn find_step(&mut self, older: bool) {
        if let Some(f) = &mut self.find {
            f.search.step(older);
        }
        self.refresh_find();
    }

    /// A key while the find box is open: all keys are its.
    pub(super) fn find_key(&mut self, event: &winit::event::KeyEvent) {
        let PhysicalKey::Code(code) = event.physical_key else { return };
        let mods = self.mods;
        match code {
            KeyCode::Escape => return self.close_find(),
            KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::F3 => return self.find_step(!mods.shift_key()),
            KeyCode::ArrowUp => return self.find_step(true),
            KeyCode::ArrowDown => return self.find_step(false),
            KeyCode::KeyF if mods.control_key() => return self.open_find(),
            _ => {}
        }
        let pasted = if mods.control_key() && code == KeyCode::KeyV { self.clip_get(false) } else { None };
        let copied: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
        let Some(f) = &mut self.find else { return };
        let out = f.field.key(code, mods, event.text.as_deref(), &mut || pasted.clone(), &mut |s| *copied.borrow_mut() = Some(s));
        if out == Outcome::Changed {
            f.search.set(&f.field.string());
            // The nearest match, from the bottom up.
            f.search.step(true);
        }
        if let Some(s) = copied.into_inner() {
            self.clip_set(s, false);
        }
        if out != Outcome::Ignored {
            self.refresh_find();
        }
    }

    /// The matches to light in pane `id`, if find is open on it.
    pub(super) fn find_spans(&self, id: PaneId, rows: u16, cols: u16) -> (Vec<Span>, Vec<Span>) {
        match &self.find {
            Some(f) if f.pane == id && !f.field.text.is_empty() => f.search.on_screen(rows, cols),
            _ => (Vec::new(), Vec::new()),
        }
    }

    /// The colours of a cell in a match: the picked one's, or any match's.
    pub(super) fn match_colors(all: &[Span], picked: &[Span], x: u16, y: u16, fg: u32, bg: u32) -> (u32, u32) {
        let inside = |s: &&Span| s.y == y && x >= s.x0 && x <= s.x1;
        if picked.iter().any(|s| inside(&s)) {
            (on(PICKED), PICKED)
        } else if all.iter().any(|s| inside(&s)) {
            (on(MATCH), mix(bg, MATCH, 200))
        } else {
            (fg, bg)
        }
    }

    /// Output came into the pane being searched: the search takes it in.
    pub(super) fn find_output(&mut self, id: PaneId) {
        if let Some(f) = self.find.as_mut().filter(|f| f.pane == id && !f.field.text.is_empty()) {
            f.search.update();
        }
    }

    pub(super) fn find_click(&mut self) -> bool {
        let (x, y) = self.pointer;
        let Some(f) = &self.find else { return false };
        match f.hits.iter().rev().find(|(r, _)| r.contains(x, y)).map(|(_, h)| *h) {
            Some(FindHit::Up) => self.find_step(true),
            Some(FindHit::Down) => self.find_step(false),
            Some(FindHit::Close) => self.close_find(),
            Some(FindHit::Field) => {}
            None => return false,
        }
        true
    }

    pub(super) fn find_hover(&mut self, x: i32, y: i32) -> bool {
        let Some(f) = &mut self.find else { return false };
        let hit = f.hits.iter().rev().find(|(r, _)| r.contains(x, y)).map(|(_, h)| *h);
        if f.hover != hit {
            f.hover = hit;
            self.full = true;
            self.redraw();
        }
        if hit.is_some() {
            if let Some(w) = &self.window {
                w.set_cursor(if hit == Some(FindHit::Field) { CursorIcon::Text } else { CursorIcon::Pointer });
            }
        }
        hit.is_some()
    }

    pub(super) fn draw_find(&mut self) {
        let Some(mut f) = self.find.take() else { return };
        let Some(rect) = self.panes.get(&f.pane).map(|p| p.rect) else { return };
        let c = self.theme.chrome();
        let scale = self.scale;
        let px = |v: f32| (v * scale).round() as i32;
        let ui = &mut self.fonts.ui;
        let h = ui.height + px(18.0);
        let w = px(400.0).min(rect.w - px(16.0));
        let r = Rect { x: rect.x + rect.w - w - px(14.0), y: rect.y + px(8.0), w, h };
        self.canvas.panel(r, px(8.0), c.panel, c.edge);
        let b = h - px(8.0);
        let close = Rect { x: r.x + r.w - b - px(4.0), y: r.y + px(4.0), w: b, h: b };
        let down = Rect { x: close.x - b - px(2.0), ..close };
        let up = Rect { x: down.x - b - px(2.0), ..close };
        let total = f.search.total();
        let count = if f.field.text.is_empty() {
            String::new()
        } else if total == 0 {
            "No results".to_string()
        } else {
            // Counted from the top, as you read them.
            match f.search.picked() {
                Some(i) => format!("{} of {total}", total - i),
                None => format!("? of {total}"),
            }
        };
        let count_w = ui.width(&count);
        let count_x = up.x - px(10.0) - count_w;
        let field = Rect { x: r.x + px(4.0), y: r.y + px(4.0), w: count_x - px(10.0) - r.x, h: b };
        self.canvas.round_fill(field, px(5.0), c.field, 255);
        let ty = r.y + (h - ui.height) / 2;
        if f.field.text.is_empty() {
            self.canvas.text(ui, "Find", (field.x + px(8.0)) as f32, ty, c.faint, field);
            self.canvas.fill(Rect { x: field.x + px(8.0), y: ty, w: px(2.0).max(2), h: ui.height }, c.text);
        } else {
            f.field.draw(&mut self.canvas, ui, field.x + px(8.0), ty, c.text, mix(self.accent, c.panel, 140), true, field);
        }
        let color = if total == 0 && !f.field.text.is_empty() { 0xE5484D } else { c.dim };
        self.canvas.text(ui, &count, count_x as f32, ty, color, r);
        // The arrows and the close button, drawn from strokes.
        for (rect, hit) in [(up, FindHit::Up), (down, FindHit::Down), (close, FindHit::Close)] {
            if f.hover == Some(hit) {
                self.canvas.round_fill(rect, px(5.0), c.hover, 255);
            }
            let (cx, cy) = (rect.x + rect.w / 2, rect.y + rect.h / 2);
            let arm = px(4.0);
            let dot = px(2.0).max(2);
            for t in -arm..=arm {
                let pts = match hit {
                    FindHit::Up => [(cx + t, cy + t.abs() / 1 - arm / 2), (cx + t, cy + t.abs() - arm / 2)],
                    FindHit::Down => [(cx + t, cy - t.abs() + arm / 2), (cx + t, cy - t.abs() + arm / 2)],
                    _ => [(cx + t, cy + t), (cx + t, cy - t)],
                };
                for (x, y) in pts {
                    self.canvas.round_fill(Rect { x: x - dot / 2, y: y - dot / 2, w: dot, h: dot }, dot / 2, c.dim, 255);
                }
            }
        }
        f.hits = vec![(r, FindHit::Field), (up, FindHit::Up), (down, FindHit::Down), (close, FindHit::Close)];
        self.find = Some(f);
        self.damage.push(Rect { x: r.x - px(16.0), y: r.y - px(16.0), w: r.w + px(32.0), h: r.h + px(32.0) });
    }
}
