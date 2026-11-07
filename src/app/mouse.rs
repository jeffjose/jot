//! The mouse given to the program in a pane when it asks for it, as htop,
//! nvim and Claude Code do: clicks, drags and the wheel. Shift held keeps
//! the mouse jterm's, to select.

use super::*;
use crate::vt::MouseIn;

impl App {
    /// Whether the pane under a point takes the mouse now.
    pub(super) fn mouse_to_program(&self, id: PaneId) -> bool {
        !self.mods.shift_key() && self.panes.get(&id).is_some_and(|p| p.term.wants_mouse())
    }

    /// Sends the mouse to pane `id`'s program. `button` is libghostty's.
    pub(super) fn send_mouse(&mut self, id: PaneId, action: sys::GhosttyMouseAction, button: Option<sys::GhosttyMouseButton>) {
        let (cw, ch) = (self.fonts.cell_w as u32, self.fonts.cell_h as u32);
        let m = self.mods;
        let mut mods = 0u16;
        for (on, bit) in [(m.shift_key(), sys::GHOSTTY_MODS_SHIFT), (m.control_key(), sys::GHOSTTY_MODS_CTRL), (m.alt_key(), sys::GHOSTTY_MODS_ALT)] {
            if on {
                mods |= bit as u16;
            }
        }
        let held = self.mouse_held.is_some();
        let Some(p) = self.panes.get_mut(&id) else { return };
        let (x, y) = ((self.pointer.0 - p.content.x) as f32, (self.pointer.1 - p.content.y) as f32);
        let input = MouseIn { action, button, x, y, mods, width: p.cols as u32 * cw, height: p.rows as u32 * ch, cell: (cw, ch), held };
        let bytes = p.term.encode_mouse(&input);
        if !bytes.is_empty() {
            p.pty.write(&bytes);
        }
    }

    pub(super) fn button_of(b: MouseButton) -> Option<sys::GhosttyMouseButton> {
        match b {
            MouseButton::Left => Some(sys::GHOSTTY_MOUSE_BUTTON_LEFT),
            MouseButton::Right => Some(sys::GHOSTTY_MOUSE_BUTTON_RIGHT),
            MouseButton::Middle => Some(sys::GHOSTTY_MOUSE_BUTTON_MIDDLE),
            _ => None,
        }
    }
}
