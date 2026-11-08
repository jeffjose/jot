//! The long-command editor: a command line in a box of its own, over the
//! pane it came from, to break over lines, edit as a text editor does,
//! and send back to the prompt or run.
//!
//! It belongs to its pane: while that pane is the one you are in, the
//! keys and clicks there are the editor's; the other panes go on as ever,
//! to click into, type in and come back from. Each pane can have one.
//!
//! In parts: the text and its editing (here), what each part of the
//! command is (`syntax`, which colors it now and is there for more:
//! completing, explaining, linking), and its drawing (`view`).

mod syntax;
mod view;

use super::*;

/// A command line being edited in a box over its pane.
pub struct Editor {
    pub pane: PaneId,
    pub lines: Vec<Vec<char>>,
    pub row: usize,
    pub col: usize,
    /// What is made goes to the pane's command line, not its prompt.
    pub into_bar: bool,
    /// Where a selection starts, row and column; it ends at the cursor.
    pub anchor: Option<(usize, usize)>,
    /// Where it was last drawn: its box, and the top left of row 0,
    /// column 0 of its text.
    pub at: Rect,
    pub origin: (i32, i32),
    /// A drag selecting; its last clicks, for double and triple ones.
    pub drag: bool,
    pub clicks: Option<(Instant, u8)>,
    /// Its Run button, where it was drawn, and whether the pointer is on it.
    pub run_at: Rect,
    pub run_lit: bool,
}

impl Editor {
    fn new(pane: PaneId, lines: Vec<Vec<char>>, into_bar: bool) -> Editor {
        let row = lines.len() - 1;
        let col = lines[row].len();
        Editor { pane, lines, row, col, into_bar, anchor: None, at: Rect::default(), origin: (0, 0), drag: false, clicks: None, run_at: Rect::default(), run_lit: false }
    }

    /// The command as the shell gets it: the lines joined by spaces.
    pub fn command(&self) -> String {
        self.lines.iter().map(|l| l.iter().collect::<String>().trim().to_string()).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ")
    }

    /// What each part of it is, line by line.
    pub fn tokens(&self) -> Vec<Vec<syntax::Token>> {
        syntax::tokens(&self.lines)
    }
}

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
    /// The editor of the pane you are in, if it has one: the one keys go
    /// to.
    pub(super) fn editor(&self) -> Option<&Editor> {
        let f = self.focused()?;
        self.editors.iter().find(|e| e.pane == f)
    }

    pub(super) fn editor_mut(&mut self) -> Option<&mut Editor> {
        let f = self.focused()?;
        self.editors.iter_mut().find(|e| e.pane == f)
    }

    /// Whether an editor's pane is on screen, so it is drawn and can be
    /// clicked.
    fn editor_shown(&self, e: &Editor) -> bool {
        !self.roam_shown && self.ws().root.contains(e.pane) && !self.ws().minimized.contains(&e.pane) && self.ws().zoom.is_none_or(|z| z == e.pane)
    }

    /// The editor drawn under a point, by its place among them.
    fn editor_under(&self, x: i32, y: i32) -> Option<usize> {
        self.editors.iter().position(|e| self.editor_shown(e) && e.at.contains(x, y))
    }

    /// The place in an editor's text at a point of the window, if the
    /// point is in its text (or anywhere, without `inside`).
    fn editor_pos_at(&self, e: &Editor, x: i32, y: i32, inside: bool) -> Option<Pos> {
        let (cw, ch) = (self.fonts.cell_w, self.fonts.cell_h);
        let (ox, oy) = e.origin;
        if inside && (y < oy || y >= oy + e.lines.len() as i32 * ch || x < ox - cw || !e.at.contains(x, y)) {
            return None;
        }
        let row = ((y - oy).max(0) / ch).min(e.lines.len() as i32 - 1) as usize;
        let col = (((x - ox) + cw / 2).max(0) / cw) as usize;
        Some((row, col.min(e.lines[row].len())))
    }

    /// Ctrl Shift E: the command line being typed, in an editor of its
    /// own; the one the pane has already, if it has one.
    pub(super) fn open_editor(&mut self) {
        let Some(id) = self.focused() else { return };
        if self.editors.iter().any(|e| e.pane == id) {
            return;
        }
        let text = self.prompt_line(id);
        self.open_editor_with(id, &text, false);
    }

    /// A command in the pane's editor (in place of what it had), broken
    /// where its options and pipes start; what is made of it goes back to
    /// the prompt, or with `into_bar` to the pane's command line.
    pub(super) fn open_editor_with(&mut self, id: PaneId, text: &str, into_bar: bool) {
        let mut lines: Vec<Vec<char>> = vec![Vec::new()];
        let words: Vec<&str> = text.trim().split(' ').collect();
        for (i, w) in words.iter().enumerate() {
            let breaks = i > 0 && (w.starts_with('-') || ["|", "&&", "||", ";"].contains(w)) && lines.last().is_some_and(|l| l.len() > 12);
            if breaks {
                lines.push(Vec::new());
            } else if i > 0 {
                lines.last_mut().unwrap().push(' ');
            }
            lines.last_mut().unwrap().extend(w.chars());
        }
        self.editors.retain(|e| e.pane != id);
        self.editors.push(Editor::new(id, lines, into_bar));
        if self.focused() != Some(id) {
            self.set_focus(id);
        }
        self.full = true;
        self.redraw();
    }

    /// The pane's editor gone, what it held with it.
    pub(super) fn close_editor(&mut self, id: PaneId) {
        self.editors.retain(|e| e.pane != id);
        self.full = true;
        self.redraw();
    }

    /// The edited line back where it came from: at the prompt, in place of
    /// what was there, or in the pane's command line. Run, with `run`.
    pub(super) fn submit_editor(&mut self, run: bool) {
        let Some(f) = self.focused() else { return };
        let Some(i) = self.editors.iter().position(|e| e.pane == f) else { return };
        let e = self.editors.remove(i);
        let line = e.command();
        if e.into_bar {
            if self.bar.as_ref().is_none_or(|b| b.pane != e.pane) {
                self.bar = None;
                self.set_focus(e.pane);
                self.toggle_bar();
            }
            if let Some(b) = &mut self.bar {
                b.field = crate::field::TextField::new(&line);
                b.selected = 0;
            }
            if run {
                self.bar_submit();
            }
            self.full = true;
            return self.relayout();
        }
        if let Some(p) = self.panes.get_mut(&e.pane) {
            // To the end of the line, and all of it taken away (Ctrl E,
            // Ctrl U), then the new line typed.
            let mut bytes = vec![0x05, 0x15];
            bytes.extend_from_slice(line.as_bytes());
            if run {
                bytes.push(b'\r');
                p.shell.entered = Some(line.clone());
            }
            p.pty.write(&bytes);
            p.shell.line = if run { None } else { Some(line) };
        }
        self.full = true;
        self.redraw();
    }

    /// A key while the pane you are in has an editor: its, but for those
    /// that go about the window (to another pane or workspace, the
    /// palette): false for those, to be done as ever.
    pub(super) fn editor_key(&mut self, event: &winit::event::KeyEvent) -> bool {
        let PhysicalKey::Code(code) = event.physical_key else { return true };
        let (ctrl, shift, alt) = (self.mods.control_key(), self.mods.shift_key(), self.mods.alt_key());
        let about = alt && !ctrl && matches!(code, KeyCode::KeyH | KeyCode::KeyJ | KeyCode::KeyK | KeyCode::KeyL | KeyCode::KeyV | KeyCode::KeyS | KeyCode::Digit1 | KeyCode::Digit2 | KeyCode::Digit3 | KeyCode::Digit4 | KeyCode::Digit5 | KeyCode::Digit6 | KeyCode::Digit7 | KeyCode::Digit8 | KeyCode::Digit9 | KeyCode::Comma | KeyCode::Period | KeyCode::Quote)
            || ctrl && !alt && matches!(code, KeyCode::KeyP | KeyCode::KeyK | KeyCode::Comma | KeyCode::KeyT | KeyCode::KeyN);
        if about {
            return false;
        }
        match code {
            KeyCode::Escape => {
                if let Some(f) = self.focused() {
                    self.close_editor(f);
                }
                return true;
            }
            KeyCode::Enter | KeyCode::NumpadEnter if ctrl => {
                self.submit_editor(true);
                return true;
            }
            KeyCode::Enter | KeyCode::NumpadEnter if shift => {
                self.submit_editor(false);
                return true;
            }
            _ => {}
        }
        let pasted = if ctrl && code == KeyCode::KeyV { self.clip_get(false) } else { None };
        let copied: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
        let mods = self.mods;
        if let Some(e) = self.editor_mut() {
            let copied = &copied;
            e.key(code, mods, event.text.as_deref(), &mut || pasted.clone(), &mut |t| *copied.borrow_mut() = Some(t));
        }
        if let Some(t) = copied.into_inner() {
            self.clip_set(t, false);
        }
        self.full = true;
        self.redraw();
        true
    }

    /// A press on an editor, or on a pane that has one: that pane gone to,
    /// and in the editor's text the cursor there, or with Shift the
    /// selection to there; twice, the word; three times, the line. The
    /// pane's terminal under it gets nothing: the editor holds it.
    pub(super) fn editor_press(&mut self) -> bool {
        let (x, y) = self.pointer;
        // A divider's reach, into the pane: the divider's.
        if self.editor_under(x, y).is_none() && self.divider_at(x, y).is_some() {
            return false;
        }
        let Some(i) = self.editor_under(x, y) else {
            // The pane around the editor: gone to, no more.
            let holder = self.pane_at(x, y).map(|(id, _)| id).filter(|id| self.editors.iter().any(|e| e.pane == *id));
            if let Some(id) = holder {
                if self.focused() != Some(id) {
                    self.set_focus(id);
                }
                self.full = true;
                self.redraw();
                return true;
            }
            return false;
        };
        let pane = self.editors[i].pane;
        if self.focused() != Some(pane) {
            self.set_focus(pane);
        }
        let at = self.editor_pos_at(&self.editors[i], x, y, true);
        let shift = self.mods.shift_key();
        let e = &mut self.editors[i];
        if let Some(at) = at {
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
        }
        self.full = true;
        self.redraw();
        true
    }

    pub(super) fn editor_drag(&mut self, x: i32, y: i32) -> bool {
        let Some(i) = self.editors.iter().position(|e| e.drag) else { return false };
        let Some(at) = self.editor_pos_at(&self.editors[i], x, y, false) else { return false };
        let e = &mut self.editors[i];
        if e.pos() != at {
            e.row = at.0;
            e.col = at.1;
            self.full = true;
            self.redraw();
        }
        true
    }

    pub(super) fn editor_release(&mut self) {
        for e in &mut self.editors {
            if std::mem::take(&mut e.drag) && e.anchor == Some(e.pos()) {
                e.anchor = None;
            }
        }
    }

    /// The pointer over an editor: the text cursor over its text, the
    /// arrow on the rest of it and on the pane it holds.
    pub(super) fn editor_hover(&mut self, x: i32, y: i32) -> bool {
        let icon = match self.editor_under(x, y) {
            Some(i) if self.editor_pos_at(&self.editors[i], x, y, true).is_some() => CursorIcon::Text,
            Some(_) => CursorIcon::Default,
            None if self.pane_at(x, y).is_some_and(|(id, _)| self.editors.iter().any(|e| e.pane == id)) => CursorIcon::Default,
            None => return false,
        };
        if let Some(w) = &self.window {
            w.set_cursor(icon);
        }
        true
    }

    /// A right click in an editor's text: its pane gone to, and cut, copy,
    /// paste, select all.
    pub(super) fn editor_menu(&mut self) -> bool {
        let (x, y) = self.pointer;
        let Some(i) = self.editor_under(x, y).filter(|i| self.editor_pos_at(&self.editors[*i], x, y, true).is_some()) else { return false };
        let pane = self.editors[i].pane;
        if self.focused() != Some(pane) {
            self.set_focus(pane);
        }
        let has = self.editors[i].selection().is_some();
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
        let Some(e) = self.editor_mut() else { return };
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

    /// A click on an editor's Run button: its pane gone to, and run, as
    /// Ctrl Enter.
    pub(super) fn editor_run_click(&mut self) -> bool {
        let (x, y) = self.pointer;
        let Some(pane) = self.editors.iter().find(|e| self.editor_shown(e) && e.run_at.contains(x, y)).map(|e| e.pane) else { return false };
        if self.focused() != Some(pane) {
            self.set_focus(pane);
        }
        self.submit_editor(true);
        true
    }

    /// The pointer over a Run button: lit, and the hand.
    pub(super) fn editor_run_hover(&mut self, x: i32, y: i32) -> bool {
        let shown: Vec<bool> = self.editors.iter().map(|e| self.editor_shown(e)).collect();
        let mut on_any = false;
        let mut changed = false;
        for (e, shown) in self.editors.iter_mut().zip(shown) {
            let on = shown && e.run_at.contains(x, y);
            if on != e.run_lit {
                e.run_lit = on;
                changed = true;
            }
            on_any |= on;
        }
        if changed {
            self.full = true;
            self.redraw();
        }
        if on_any {
            if let Some(w) = &self.window {
                w.set_cursor(CursorIcon::Pointer);
            }
        }
        on_any
    }
}
