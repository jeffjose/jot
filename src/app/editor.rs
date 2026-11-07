//! The long-command editor's editing, as a plain text editor has it: a
//! selection by Shift and the arrows or by the mouse, Ctrl A, cut, copy
//! and paste, words a Ctrl or Alt at a time, typing over a selection.

use super::commands::Editor;
use super::*;

type Pos = (usize, usize);

impl Editor {
    fn pos(&self) -> Pos {
        (self.row, self.col)
    }

    /// The selection, start before end, when there is one.
    pub fn selection(&self) -> Option<(Pos, Pos)> {
        let a = self.anchor?;
        let b = self.pos();
        (a != b).then(|| if a < b { (a, b) } else { (b, a) })
    }

    pub fn selected(&self) -> Option<String> {
        let ((r0, c0), (r1, c1)) = self.selection()?;
        let mut out = String::new();
        for r in r0..=r1 {
            let line = &self.lines[r];
            let from = if r == r0 { c0 } else { 0 };
            let to = if r == r1 { c1 } else { line.len() };
            out.extend(&line[from.min(line.len())..to.min(line.len())]);
            if r < r1 {
                out.push('\n');
            }
        }
        Some(out)
    }

    /// The selection taken out; true when there was one.
    pub fn delete_selection(&mut self) -> bool {
        let Some(((r0, c0), (r1, c1))) = self.selection() else {
            self.anchor = None;
            return false;
        };
        let tail: Vec<char> = self.lines[r1][c1..].to_vec();
        self.lines[r0].truncate(c0);
        self.lines[r0].extend(tail);
        self.lines.drain(r0 + 1..=r1);
        self.row = r0;
        self.col = c0;
        self.anchor = None;
        true
    }

    /// Text put in at the cursor, over the selection; new lines split.
    pub fn insert(&mut self, text: &str) {
        self.delete_selection();
        for c in text.chars() {
            if c == '\n' {
                let rest = self.lines[self.row].split_off(self.col);
                self.lines.insert(self.row + 1, rest);
                self.row += 1;
                self.col = 0;
            } else if !c.is_control() {
                self.lines[self.row].insert(self.col, c);
                self.col += 1;
            }
        }
    }

    /// The cursor to a place; with `extend`, the selection to it.
    pub fn go(&mut self, to: Pos, extend: bool) {
        if extend {
            self.anchor.get_or_insert((self.row, self.col));
        } else {
            self.anchor = None;
        }
        self.row = to.0.min(self.lines.len() - 1);
        self.col = to.1.min(self.lines[self.row].len());
    }

    pub fn select_all(&mut self) {
        self.anchor = Some((0, 0));
        self.row = self.lines.len() - 1;
        self.col = self.lines[self.row].len();
    }

    /// The word around a place selected.
    pub fn select_word(&mut self, at: Pos) {
        let line = &self.lines[at.0];
        let mut a = at.1.min(line.len());
        while a > 0 && !line[a - 1].is_whitespace() {
            a -= 1;
        }
        let mut b = at.1.min(line.len());
        while b < line.len() && !line[b].is_whitespace() {
            b += 1;
        }
        self.anchor = Some((at.0, a));
        self.row = at.0;
        self.col = b;
    }

    pub fn select_line(&mut self, row: usize) {
        self.anchor = Some((row, 0));
        self.row = row;
        self.col = self.lines[row].len();
    }

    /// Where the word before the cursor starts, across lines.
    fn word_left(&self) -> Pos {
        let (mut r, mut c) = self.pos();
        if c == 0 {
            return if r > 0 { (r - 1, self.lines[r - 1].len()) } else { (0, 0) };
        }
        let line = &self.lines[r];
        while c > 0 && line[c - 1].is_whitespace() {
            c -= 1;
        }
        while c > 0 && !line[c - 1].is_whitespace() {
            c -= 1;
        }
        let _ = &mut r;
        (r, c)
    }

    fn word_right(&self) -> Pos {
        let (r, mut c) = self.pos();
        let line = &self.lines[r];
        if c >= line.len() {
            return if r + 1 < self.lines.len() { (r + 1, 0) } else { (r, c) };
        }
        while c < line.len() && line[c].is_whitespace() {
            c += 1;
        }
        while c < line.len() && !line[c].is_whitespace() {
            c += 1;
        }
        (r, c)
    }

    fn left(&self) -> Pos {
        match self.pos() {
            (r, 0) if r > 0 => (r - 1, self.lines[r - 1].len()),
            (r, c) => (r, c.saturating_sub(1)),
        }
    }

    fn right(&self) -> Pos {
        let (r, c) = self.pos();
        if c < self.lines[r].len() {
            (r, c + 1)
        } else if r + 1 < self.lines.len() {
            (r + 1, 0)
        } else {
            (r, c)
        }
    }

    /// A key; what is cut or copied goes to `set`, what is pasted comes
    /// from `get`.
    pub fn key(&mut self, code: KeyCode, mods: winit::keyboard::ModifiersState, text: Option<&str>, get: &mut dyn FnMut() -> Option<String>, set: &mut dyn FnMut(String)) {
        let (ctrl, shift, alt) = (mods.control_key(), mods.shift_key(), mods.alt_key());
        let word = ctrl || alt;
        match code {
            KeyCode::KeyA if ctrl => self.select_all(),
            KeyCode::KeyC if ctrl => {
                if let Some(s) = self.selected() {
                    set(s);
                }
            }
            KeyCode::KeyX if ctrl => {
                if let Some(s) = self.selected() {
                    set(s);
                    self.delete_selection();
                }
            }
            KeyCode::KeyV if ctrl => {
                if let Some(s) = get() {
                    self.insert(&s);
                }
            }
            KeyCode::Enter | KeyCode::NumpadEnter => self.insert("\n"),
            KeyCode::Backspace => {
                if !self.delete_selection() {
                    self.anchor = Some(self.pos());
                    let to = if word { self.word_left() } else { self.left() };
                    self.go(to, true);
                    self.delete_selection();
                }
            }
            KeyCode::Delete => {
                if !self.delete_selection() {
                    self.anchor = Some(self.pos());
                    let to = if word { self.word_right() } else { self.right() };
                    self.go(to, true);
                    self.delete_selection();
                }
            }
            KeyCode::ArrowLeft => {
                let to = match (word, self.selection()) {
                    (false, Some((a, _))) if !shift => a,
                    (true, _) => self.word_left(),
                    _ => self.left(),
                };
                self.go(to, shift);
            }
            KeyCode::ArrowRight => {
                let to = match (word, self.selection()) {
                    (false, Some((_, b))) if !shift => b,
                    (true, _) => self.word_right(),
                    _ => self.right(),
                };
                self.go(to, shift);
            }
            KeyCode::ArrowUp => {
                let to = if self.row > 0 { (self.row - 1, self.col) } else { (0, 0) };
                self.go(to, shift);
            }
            KeyCode::ArrowDown => {
                let last = self.lines.len() - 1;
                let to = if self.row < last { (self.row + 1, self.col) } else { (last, self.lines[last].len()) };
                self.go(to, shift);
            }
            KeyCode::Home if ctrl => self.go((0, 0), shift),
            KeyCode::End if ctrl => {
                let last = self.lines.len() - 1;
                self.go((last, self.lines[last].len()), shift);
            }
            KeyCode::Home => self.go((self.row, 0), shift),
            KeyCode::End => self.go((self.row, self.lines[self.row].len()), shift),
            // Alt B, F, D: a word back, forward, and the next word gone.
            KeyCode::KeyB if alt && !ctrl => {
                let to = self.word_left();
                self.go(to, shift);
            }
            KeyCode::KeyF if alt && !ctrl => {
                let to = self.word_right();
                self.go(to, shift);
            }
            KeyCode::KeyD if alt && !ctrl => {
                self.anchor = Some(self.pos());
                let to = self.word_right();
                self.go(to, true);
                self.delete_selection();
            }
            _ => {
                if !ctrl && !alt {
                    if let Some(t) = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control)) {
                        self.insert(t);
                    }
                }
            }
        }
    }
}

impl App {
    /// The place in the editor's text at a point of the window, if the
    /// point is in its text.
    fn editor_pos_at(&self, x: i32, y: i32, inside: bool) -> Option<Pos> {
        let e = self.editor.as_ref()?;
        let (cw, ch) = (self.fonts.cell_w, self.fonts.cell_h);
        let (ox, oy) = e.origin;
        if inside && (y < oy || y >= oy + e.lines.len() as i32 * ch || x < ox - cw) {
            return None;
        }
        let row = ((y - oy).max(0) / ch).min(e.lines.len() as i32 - 1) as usize;
        let col = (((x - ox) + cw / 2).max(0) / cw) as usize;
        Some((row, col.min(e.lines[row].len())))
    }

    /// A press in the editor's text: the cursor there, or with Shift the
    /// selection to there; twice, the word; three times, the line.
    pub(super) fn editor_press(&mut self) -> bool {
        let (x, y) = self.pointer;
        let Some(at) = self.editor_pos_at(x, y, true) else { return false };
        let shift = self.mods.shift_key();
        let Some(e) = &mut self.editor else { return false };
        let n = match e.clicks {
            Some((t, n)) if t.elapsed() < Duration::from_millis(400) => n % 3 + 1,
            _ => 1,
        };
        e.clicks = Some((Instant::now(), n));
        match n {
            2 => e.select_word(at),
            3 => e.select_line(at.0),
            _ => {
                e.go(at, shift);
                e.anchor.get_or_insert(at);
            }
        }
        e.drag = n == 1;
        self.full = true;
        self.redraw();
        true
    }

    pub(super) fn editor_drag(&mut self, x: i32, y: i32) -> bool {
        if !self.editor.as_ref().is_some_and(|e| e.drag) {
            return false;
        }
        let Some(at) = self.editor_pos_at(x, y, false) else { return false };
        if let Some(e) = &mut self.editor {
            if e.pos() != at {
                e.row = at.0;
                e.col = at.1;
                self.full = true;
                self.redraw();
            }
        }
        true
    }

    pub(super) fn editor_release(&mut self) {
        if let Some(e) = &mut self.editor {
            if std::mem::take(&mut e.drag) && e.anchor == Some(e.pos()) {
                e.anchor = None;
            }
        }
    }

    /// The pointer over the editor's text: the text cursor.
    pub(super) fn editor_hover(&mut self, x: i32, y: i32) -> bool {
        let over = self.editor_pos_at(x, y, true).is_some();
        if over {
            if let Some(w) = &self.window {
                w.set_cursor(CursorIcon::Text);
            }
        }
        over
    }

    /// A right click in the editor: cut, copy, paste, select all.
    pub(super) fn editor_menu(&mut self) -> bool {
        let (x, y) = self.pointer;
        if self.editor_pos_at(x, y, true).is_none() {
            return false;
        }
        let has = self.editor.as_ref().is_some_and(|e| e.selection().is_some());
        let c = |label: &str, hint: &str, action| Some(crate::palette::Item { label: label.into(), hint: hint.into(), action });
        use crate::palette::Action;
        self.open_items_menu(vec![
            if has { c("Cut", "Ctrl X", Action::FieldCut) } else { None },
            if has { c("Copy", "Ctrl C", Action::FieldCopy) } else { None },
            c("Paste", "Ctrl V", Action::FieldPaste),
            None,
            c("Select all", "Ctrl A", Action::FieldSelectAll),
        ]);
        true
    }

    /// What the editor's menu chose.
    pub(super) fn editor_menu_do(&mut self, action: &crate::palette::Action) {
        use crate::palette::Action;
        let pasted = if *action == Action::FieldPaste { self.clip_get(false) } else { None };
        let Some(e) = &mut self.editor else { return };
        let mut copied = None;
        match action {
            Action::FieldCut => {
                copied = e.selected();
                e.delete_selection();
            }
            Action::FieldCopy => copied = e.selected(),
            Action::FieldPaste => {
                if let Some(s) = pasted {
                    e.insert(&s);
                }
            }
            Action::FieldSelectAll => e.select_all(),
            _ => {}
        }
        if let Some(s) = copied {
            self.clip_set(s, false);
        }
        self.full = true;
        self.redraw();
    }
}

impl App {
    /// A click on the editor's Run button: run, as Ctrl Enter.
    pub(super) fn editor_run_click(&mut self) -> bool {
        let (x, y) = self.pointer;
        if self.editor.as_ref().is_some_and(|e| e.run_at.contains(x, y)) {
            self.submit_editor(true);
            return true;
        }
        false
    }

    /// The pointer over the Run button: lit, and the hand.
    pub(super) fn editor_run_hover(&mut self, x: i32, y: i32) -> bool {
        let Some(e) = &mut self.editor else { return false };
        let on = e.run_at.contains(x, y);
        if on != e.run_lit {
            e.run_lit = on;
            self.full = true;
            self.redraw();
        }
        if on {
            if let Some(w) = &self.window {
                w.set_cursor(CursorIcon::Pointer);
            }
        }
        on
    }
}
