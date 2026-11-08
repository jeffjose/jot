//! Renaming in place: a terminal's name in its sidebar row or on the quake
//! window's bar, a workspace's on the project bar, edited where it is, not
//! in a box of its own. Double-clicked, or Rename… from its menu. Enter
//! keeps it, Escape leaves it as it was, a click elsewhere keeps it; a
//! terminal's name emptied is its own again.

use super::*;

/// What is being renamed.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Target {
    Terminal(usize, u64),
    /// A workspace, by its id.
    Workspace(u64),
}

pub(super) struct Inline {
    pub target: Target,
    pub field: crate::field::TextField,
    /// Where it is drawn, for a click in it.
    pub at: Option<Rect>,
}

/// Two clicks on one name this close together: a double click.
const DOUBLE: Duration = Duration::from_millis(400);

impl App {
    /// Terminal `sid` of machine `k` renamed in place: where its name is
    /// drawn (its row in the sidebar, or its entry on the quake bar), else
    /// in the box.
    pub(super) fn rename_terminal(&mut self, k: usize, sid: u64) {
        let shown = if crate::quake_mode() { self.quake_bar_h() > 0 } else { self.sidebar_open };
        if !shown {
            return self.rename_terminal_prompt(k, sid);
        }
        let name = self.given_name(k, sid).or_else(|| self.side_terminals().into_iter().find(|t| (t.k, t.sid) == (k, sid)).map(|t| t.label)).unwrap_or_default();
        self.start_inline(Target::Terminal(k, sid), &name);
    }

    /// The workspace you are in renamed in place, on the project bar; in
    /// the box when there is no bar.
    pub(super) fn rename_workspace(&mut self) -> bool {
        if self.foot_h() == 0 || self.workspaces.is_empty() {
            return false;
        }
        let (id, name) = (self.ws().id, self.ws().name.clone());
        self.start_inline(Target::Workspace(id), &name);
        true
    }

    fn start_inline(&mut self, target: Target, name: &str) {
        let mut field = crate::field::TextField::new(name);
        field.select_all();
        self.inline = Some(Inline { target, field, at: None });
        self.menu = None;
        self.full = true;
        self.foot_said.clear();
        self.redraw();
    }

    /// A click on a name: twice on the same one, renamed there.
    pub(super) fn inline_click(&mut self, target: Target) -> bool {
        let again = self.inline_clicked.is_some_and(|(t, at)| t == target && at.elapsed() < DOUBLE);
        self.inline_clicked = Some((target, Instant::now()));
        if !again {
            return false;
        }
        self.inline_clicked = None;
        match target {
            Target::Terminal(k, sid) => self.rename_terminal(k, sid),
            Target::Workspace(_) => {
                self.rename_workspace();
            }
        }
        true
    }

    /// A key while a name is being edited: the field's, Enter keeping it,
    /// Escape leaving it as it was.
    pub(super) fn inline_key(&mut self, event: &winit::event::KeyEvent) {
        let PhysicalKey::Code(code) = event.physical_key else { return };
        match code {
            KeyCode::Enter | KeyCode::NumpadEnter => return self.inline_done(true),
            KeyCode::Escape => return self.inline_done(false),
            _ => {}
        }
        let pasted = if self.mods.control_key() && code == KeyCode::KeyV { self.clip_get(false) } else { None };
        let copied = std::cell::RefCell::new(None);
        let mods = self.mods;
        if let Some(i) = &mut self.inline {
            i.field.key(code, mods, event.text.as_deref(), &mut || pasted.clone(), &mut |t| *copied.borrow_mut() = Some(t));
        }
        if let Some(s) = copied.into_inner() {
            self.clip_set(s, false);
        }
        self.full = true;
        self.foot_said.clear();
        self.redraw();
    }

    /// The name kept (`keep`) or left as it was; the field gone.
    pub(super) fn inline_done(&mut self, keep: bool) {
        let Some(i) = self.inline.take() else { return };
        self.full = true;
        self.foot_said.clear();
        if keep {
            let name = i.field.string().trim().to_string();
            match i.target {
                Target::Terminal(k, sid) => self.name_terminal(k, sid, &name),
                Target::Workspace(id) => {
                    if let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == id).filter(|_| !name.is_empty()) {
                        ws.name = name;
                        self.unsaved = true;
                        self.set_title();
                    }
                }
            }
        }
        self.redraw();
    }

    /// A press while a name is being edited: in it, the cursor put there;
    /// elsewhere, the name kept, and the press goes on to what it is on.
    pub(super) fn inline_press(&mut self) -> bool {
        let (x, y) = self.pointer;
        let Some(at) = self.inline.as_ref().map(|i| i.at) else { return false };
        match at {
            Some(r) if r.contains(x, y) => {
                let font = if matches!(self.inline.as_ref().map(|i| i.target), Some(Target::Terminal(..))) && !crate::quake_mode() { &mut self.fonts.side } else { &mut self.fonts.ui };
                if let Some(i) = &mut self.inline {
                    let at = i.field.index_at(font, x - r.x);
                    i.field.place(at, false);
                }
                self.full = true;
                self.foot_said.clear();
                self.redraw();
                true
            }
            _ => {
                self.inline_done(true);
                false
            }
        }
    }

    /// The field drawn for `target` at (x, y) in `font`, if it is the one
    /// being edited: a quiet box around it, the selection and the caret.
    /// Where the text ends, None when it is not being edited.
    pub(super) fn draw_inline(&mut self, target: Target, x: i32, y: i32, w: i32, side_font: bool, clip: Rect) -> Option<i32> {
        if self.inline.as_ref().map(|i| i.target) != Some(target) {
            return None;
        }
        let c = self.cur_theme.chrome();
        let font_h = if side_font { self.fonts.side.height } else { self.fonts.ui.height };
        let pad = self.px(4.0);
        let r = Rect { x: x - pad, y: y - self.px(2.0), w: w + 2 * pad, h: font_h + self.px(4.0) };
        self.canvas.round_fill(r, self.px(4.0), c.field, 255);
        self.canvas.ring(r, self.px(4.0), 1, self.cur_accent);
        let selection = mix(c.field, self.cur_accent, 110);
        let font = if side_font { &mut self.fonts.side } else { &mut self.fonts.ui };
        let i = self.inline.as_mut()?;
        i.at = Some(r);
        let inner = Rect { x, y: r.y, w, h: r.h };
        let clip = Rect { x: inner.x.max(clip.x), y: inner.y.max(clip.y), w: inner.w.min(clip.w), h: inner.h };
        let end = i.field.draw(&mut self.canvas, font, x, y, c.text, selection, true, clip);
        self.damage.push(r);
        Some(end)
    }
}
