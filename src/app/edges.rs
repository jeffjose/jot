//! The window's edges and corners, easy to catch: within 6 px of the left,
//! right and bottom edges (and the top, with no title bar above it), and
//! 12 px of a corner, the pointer says resize and a press resizes the
//! window, as the frame's own thin border would. Not when maximized or
//! full screen, nor in the quake window, which has its own bar.

use super::*;
use winit::window::ResizeDirection;

impl App {
    /// The edge or corner of the window under a point, to resize it by.
    fn edge_at(&self, x: i32, y: i32) -> Option<ResizeDirection> {
        let w = self.window.as_ref()?;
        if crate::quake_mode() || w.is_maximized() || w.fullscreen().is_some() {
            return None;
        }
        // A caption's buttons by the edge keep their clicks.
        if self.multi_tags.iter().any(|(r, _, _)| r.contains(x, y)) {
            return None;
        }
        let (edge, corner) = (self.px(6.0), self.px(12.0));
        let (width, height) = (self.canvas.width, self.canvas.height);
        let top_too = !w.is_decorated();
        let (left, right) = (x < edge, x >= width - edge);
        let (top, bottom) = (top_too && y < edge, y >= height - edge);
        let (near_left, near_right) = (x < corner, x >= width - corner);
        let (near_top, near_bottom) = (top_too && y < corner, y >= height - corner);
        use ResizeDirection::*;
        Some(match () {
            _ if (bottom && near_right) || (right && near_bottom) => SouthEast,
            _ if (bottom && near_left) || (left && near_bottom) => SouthWest,
            _ if (top && near_right) || (right && near_top) => NorthEast,
            _ if (top && near_left) || (left && near_top) => NorthWest,
            _ if right => East,
            _ if left => West,
            _ if bottom => South,
            _ if top => North,
            _ => return None,
        })
    }

    /// The pointer near an edge or corner: the resize arrows that say
    /// which way. True when it is.
    pub(super) fn edge_hover(&mut self, x: i32, y: i32) -> bool {
        let Some(dir) = self.edge_at(x, y) else { return false };
        use ResizeDirection::*;
        let icon = match dir {
            East => CursorIcon::EResize,
            West => CursorIcon::WResize,
            North => CursorIcon::NResize,
            South => CursorIcon::SResize,
            NorthEast => CursorIcon::NeResize,
            NorthWest => CursorIcon::NwResize,
            SouthEast => CursorIcon::SeResize,
            SouthWest => CursorIcon::SwResize,
        };
        if let Some(w) = &self.window {
            w.set_cursor(icon);
        }
        true
    }

    /// A press near an edge or corner: the window resized from it, the
    /// desktop taking the drag. True when it was.
    pub(super) fn edge_press(&mut self) -> bool {
        let (x, y) = self.pointer;
        let Some(dir) = self.edge_at(x, y) else { return false };
        self.window.as_ref().is_some_and(|w| w.drag_resize_window(dir).is_ok())
    }
}
