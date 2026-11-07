//! A line of text being typed into, as in the palette and the font list:
//! a cursor, a selection, and the keys a text box has everywhere else.
//! Ctrl A selects all, Ctrl C, X and V copy, cut and paste, Shift with
//! the arrows, Home and End selects, and typing replaces what is selected.

use winit::keyboard::{KeyCode, ModifiersState};

use crate::canvas::Canvas;
use crate::font::UiFont;
use crate::layout::Rect;

#[derive(Clone, Default, Debug)]
pub struct TextField {
    pub text: Vec<char>,
    pub cursor: usize,
    /// Where a selection started; it runs to the cursor.
    pub anchor: Option<usize>,
}

/// What a key did to the field.
#[derive(PartialEq, Debug)]
pub enum Outcome {
    /// The text is different.
    Changed,
    /// Only the cursor or the selection moved.
    Moved,
    /// Not a key for the field.
    Ignored,
}

impl TextField {
    pub fn new(text: &str) -> TextField {
        let text: Vec<char> = text.chars().collect();
        TextField { cursor: text.len(), text, anchor: None }
    }

    pub fn string(&self) -> String {
        self.text.iter().collect()
    }

    /// The selection, start before end, when it holds anything.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let a = self.anchor?;
        (a != self.cursor).then(|| (a.min(self.cursor), a.max(self.cursor)))
    }

    pub fn selected(&self) -> Option<String> {
        self.selection().map(|(a, b)| self.text[a..b].iter().collect())
    }

    fn delete_selection(&mut self) -> bool {
        let Some((a, b)) = self.selection() else {
            self.anchor = None;
            return false;
        };
        self.text.drain(a..b);
        self.cursor = a;
        self.anchor = None;
        true
    }

    /// Text typed or pasted in at the cursor, over what is selected. A
    /// paste of several lines comes in as one.
    pub fn insert(&mut self, s: &str) {
        self.delete_selection();
        for c in s.chars().filter(|c| !c.is_control() || *c == '\n').map(|c| if c == '\n' { ' ' } else { c }) {
            self.text.insert(self.cursor, c);
            self.cursor += 1;
        }
    }

    /// The cursor moved to `to`, selecting as it goes when `select`.
    fn go(&mut self, to: usize, select: bool) {
        if select {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = to.min(self.text.len());
    }

    /// Where the word before `at` starts.
    fn word_before(&self, at: usize) -> usize {
        let mut i = at;
        while i > 0 && self.text[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !self.text[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    fn word_after(&self, at: usize) -> usize {
        let mut i = at;
        let n = self.text.len();
        while i < n && self.text[i].is_whitespace() {
            i += 1;
        }
        while i < n && !self.text[i].is_whitespace() {
            i += 1;
        }
        i
    }

    /// A key, with the text it typed. `get` reads the clipboard and `set`
    /// writes it.
    pub fn key(&mut self, code: KeyCode, mods: ModifiersState, text: Option<&str>, get: &mut dyn FnMut() -> Option<String>, set: &mut dyn FnMut(String)) -> Outcome {
        let (ctrl, shift) = (mods.control_key(), mods.shift_key());
        // Alt works a word at a time as Ctrl does, as in a shell.
        let word = ctrl || mods.alt_key();
        let alt = mods.alt_key() && !ctrl;
        let before = self.text.clone();
        match code {
            KeyCode::KeyA if ctrl => {
                self.anchor = Some(0);
                self.cursor = self.text.len();
                return Outcome::Moved;
            }
            KeyCode::KeyC if ctrl => {
                if let Some(s) = self.selected() {
                    set(s);
                }
                return Outcome::Moved;
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
            KeyCode::KeyU if ctrl => {
                self.text.drain(..self.cursor);
                self.cursor = 0;
                self.anchor = None;
            }
            KeyCode::KeyW if ctrl => {
                if !self.delete_selection() {
                    let from = self.word_before(self.cursor);
                    self.text.drain(from..self.cursor);
                    self.cursor = from;
                }
            }
            KeyCode::Backspace => {
                if !self.delete_selection() && self.cursor > 0 {
                    let from = if word { self.word_before(self.cursor) } else { self.cursor - 1 };
                    self.text.drain(from..self.cursor);
                    self.cursor = from;
                }
            }
            KeyCode::Delete => {
                if !self.delete_selection() && self.cursor < self.text.len() {
                    let to = if word { self.word_after(self.cursor) } else { self.cursor + 1 };
                    self.text.drain(self.cursor..to);
                }
            }
            KeyCode::ArrowLeft => {
                let to = match (word, self.selection()) {
                    (false, Some((a, _))) if !shift => a,
                    (true, _) => self.word_before(self.cursor),
                    _ => self.cursor.saturating_sub(1),
                };
                self.go(to, shift);
                return Outcome::Moved;
            }
            KeyCode::ArrowRight => {
                let to = match (word, self.selection()) {
                    (false, Some((_, b))) if !shift => b,
                    (true, _) => self.word_after(self.cursor),
                    _ => self.cursor + 1,
                };
                self.go(to, shift);
                return Outcome::Moved;
            }
            // Alt B, F and D: a word back, forward, and the next word gone,
            // as tcsh and emacs have them.
            KeyCode::KeyB if alt => {
                self.go(self.word_before(self.cursor), shift);
                return Outcome::Moved;
            }
            KeyCode::KeyF if alt => {
                self.go(self.word_after(self.cursor), shift);
                return Outcome::Moved;
            }
            KeyCode::KeyD if alt => {
                let to = self.word_after(self.cursor);
                self.text.drain(self.cursor..to);
                self.anchor = None;
            }
            KeyCode::Home => {
                self.go(0, shift);
                return Outcome::Moved;
            }
            KeyCode::End => {
                self.go(self.text.len(), shift);
                return Outcome::Moved;
            }
            _ => {
                if ctrl || mods.alt_key() || mods.super_key() {
                    return Outcome::Ignored;
                }
                match text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control)) {
                    Some(t) => self.insert(t),
                    None => return Outcome::Ignored,
                }
            }
        }
        if self.text != before {
            Outcome::Changed
        } else {
            Outcome::Moved
        }
    }

    /// The field's text at `(x, y)`, the selection behind it and the caret
    /// at the cursor. Returns where the text ends.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&self, canvas: &mut Canvas, font: &mut UiFont, x: i32, y: i32, color: u32, selection: u32, caret: bool, clip: Rect) -> i32 {
        let width = |font: &mut UiFont, upto: usize| font.width(&self.text[..upto].iter().collect::<String>());
        if let Some((a, b)) = self.selection() {
            let (sx, ex) = (x + width(font, a), x + width(font, b));
            canvas.round_fill(Rect { x: sx, y: y - 1, w: (ex - sx).max(1), h: font.height + 2 }, 3, selection, 255);
        }
        let end = canvas.text(font, &self.string(), x as f32, y, color, clip);
        if caret {
            let cx = x + width(font, self.cursor);
            canvas.fill(Rect { x: cx, y, w: 2, h: font.height }, color);
        }
        end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(f: &mut TextField, code: KeyCode, mods: ModifiersState, clip: &mut String) -> Outcome {
        let c = std::cell::RefCell::new(clip.clone());
        let out = f.key(code, mods, None, &mut || Some(c.borrow().clone()), &mut |s| *c.borrow_mut() = s);
        *clip = c.into_inner();
        out
    }

    #[test]
    fn select_all_copy_cut_paste() {
        let mut f = TextField::new("hello world");
        let mut clip = String::new();
        press(&mut f, KeyCode::KeyA, ModifiersState::CONTROL, &mut clip);
        assert_eq!(f.selected().as_deref(), Some("hello world"));
        press(&mut f, KeyCode::KeyC, ModifiersState::CONTROL, &mut clip);
        assert_eq!(clip, "hello world");
        assert_eq!(press(&mut f, KeyCode::KeyX, ModifiersState::CONTROL, &mut clip), Outcome::Changed);
        assert_eq!(f.string(), "");
        clip = "a\nb".into();
        press(&mut f, KeyCode::KeyV, ModifiersState::CONTROL, &mut clip);
        assert_eq!(f.string(), "a b");
        // Typing over a selection replaces it.
        press(&mut f, KeyCode::Home, ModifiersState::SHIFT, &mut clip);
        f.key(KeyCode::KeyZ, ModifiersState::empty(), Some("z"), &mut || None, &mut |_| {});
        assert_eq!(f.string(), "z");
    }

    #[test]
    fn words() {
        let mut f = TextField::new("one two three");
        let mut clip = String::new();
        press(&mut f, KeyCode::Backspace, ModifiersState::CONTROL, &mut clip);
        assert_eq!(f.string(), "one two ");
        press(&mut f, KeyCode::ArrowLeft, ModifiersState::CONTROL, &mut clip);
        assert_eq!(f.cursor, 4);
    }
}
