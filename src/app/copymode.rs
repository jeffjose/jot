//! Copy mode, on Alt P, as screen's: a cursor moved through the pane's
//! scrollback with the keys, a selection made from it and copied, no
//! mouse. h j k l and the arrows move, 0 and $ go to a row's ends, g and
//! G to the top and the bottom, Ctrl U and D and the Page keys by half
//! and whole screens; v selects from here, V whole rows; y or Enter
//! copies and leaves; Esc or q leaves.

use super::*;

pub struct CopyMode {
    pub pane: PaneId,
    pub x: u16,
    /// The row, counted from the top of the scrollback.
    pub y: u64,
    /// Where the selection started, and whether it takes whole rows.
    pub anchor: Option<(u16, u64)>,
    pub rows: bool,
}

impl App {
    pub(super) fn open_copy_mode(&mut self) {
        let Some(id) = self.focused() else { return };
        let Some(p) = self.panes.get_mut(&id) else { return };
        let (_, offset, _) = p.term.scrollbar();
        let c = p.term.cursor();
        p.term.clear_selection();
        p.full = true;
        self.copy_mode = Some(CopyMode { pane: id, x: c.x, y: offset + c.y as u64, anchor: None, rows: false });
        self.full = true;
        self.redraw();
    }

    fn close_copy_mode(&mut self, keep: bool) {
        if let Some(c) = self.copy_mode.take() {
            if let Some(p) = self.panes.get_mut(&c.pane) {
                if !keep {
                    p.term.clear_selection();
                }
                p.term.scroll_to_bottom();
                p.full = true;
            }
        }
        self.full = true;
        self.redraw();
    }

    pub(super) fn copy_mode_key(&mut self, event: &winit::event::KeyEvent) {
        let PhysicalKey::Code(code) = event.physical_key else { return };
        let (ctrl, shift) = (self.mods.control_key(), self.mods.shift_key());
        let text = event.text.as_deref().unwrap_or("");
        let Some(mut c) = self.copy_mode.take() else { return };
        let Some(p) = self.panes.get_mut(&c.pane) else { return };
        let (total, _, len) = p.term.scrollbar();
        let last = total.saturating_sub(1);
        let cols = p.cols.saturating_sub(1);
        let half = (len / 2).max(1);
        let row_end = |p: &Pane, c: &CopyMode| -> u16 {
            // The last character of the row, if it is on screen.
            let (_, offset, _) = p.term.scrollbar();
            let vy = c.y.checked_sub(offset).map(|v| v as usize);
            vy.and_then(|v| p.lines.get(v)).and_then(|l| l.iter().rposition(|ch| !ch.is_whitespace())).map_or(cols, |i| i as u16)
        };
        match (code, text) {
            (KeyCode::Escape, _) | (_, "q") => {
                self.copy_mode = Some(c);
                return self.close_copy_mode(false);
            }
            (KeyCode::Enter, _) | (_, "y") => {
                self.copy_mode = Some(c);
                self.copy(false);
                self.copy(true);
                return self.close_copy_mode(false);
            }
            (KeyCode::KeyU, _) if ctrl => c.y = c.y.saturating_sub(half),
            (KeyCode::KeyD, _) if ctrl => c.y = (c.y + half).min(last),
            (KeyCode::KeyB, _) if ctrl => c.y = c.y.saturating_sub(len),
            (KeyCode::KeyF, _) if ctrl => c.y = (c.y + len).min(last),
            (KeyCode::PageUp, _) => c.y = c.y.saturating_sub(len),
            (KeyCode::PageDown, _) => c.y = (c.y + len).min(last),
            (KeyCode::ArrowLeft, _) | (_, "h") => c.x = c.x.saturating_sub(1),
            (KeyCode::ArrowRight, _) | (_, "l") => c.x = (c.x + 1).min(cols),
            (KeyCode::ArrowUp, _) | (_, "k") => c.y = c.y.saturating_sub(1),
            (KeyCode::ArrowDown, _) | (_, "j") => c.y = (c.y + 1).min(last),
            (KeyCode::Home, _) | (_, "0") => c.x = 0,
            (KeyCode::End, _) | (_, "$") => c.x = row_end(p, &c),
            (_, "g") => c.y = 0,
            (_, "G") => c.y = last,
            (_, "v") => {
                c.anchor = if c.anchor.is_some() && !c.rows { None } else { Some((c.x, c.y)) };
                c.rows = false;
            }
            (_, "V") => {
                c.anchor = if c.anchor.is_some() && c.rows { None } else { Some((c.x, c.y)) };
                c.rows = true;
            }
            _ => {
                let _ = shift;
                self.copy_mode = Some(c);
                return;
            }
        }
        // The view follows the cursor.
        let (_, offset, len) = p.term.scrollbar();
        if c.y < offset {
            p.term.scroll(-((offset - c.y) as isize));
        } else if c.y >= offset + len {
            p.term.scroll((c.y - (offset + len) + 1) as isize);
        }
        match c.anchor {
            Some((_, ay)) if c.rows => {
                let (top, bottom) = (ay.min(c.y), ay.max(c.y));
                p.term.select_rows((0, top), (cols, bottom));
            }
            Some(a) => p.term.select_rows(a, (c.x, c.y)),
            None => p.term.clear_selection(),
        }
        p.full = true;
        self.copy_mode = Some(c);
        self.full = true;
        self.redraw();
    }

    /// The copy cursor's cell in pane `id`'s view, if it is in copy mode.
    pub(super) fn copy_cursor(&self, id: PaneId) -> Option<(u16, u16)> {
        let c = self.copy_mode.as_ref().filter(|c| c.pane == id)?;
        let (_, offset, len) = self.panes.get(&id)?.term.scrollbar();
        (c.y >= offset && c.y < offset + len).then(|| (c.x, (c.y - offset) as u16))
    }
}
