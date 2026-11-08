//! jot's own title bar, in place of the desktop's (whose font and look
//! jot cannot choose: Xfwm's on Xubuntu, the frame's on GNOME): a slim bar
//! in the sidebar's ground and the interface font, the workspace (its
//! color, its name) and the folder of the pane you are in, and the
//! window's buttons at the right, borderless, in Solar's line icons.
//! Dragged, it moves the window; double-clicked, it maximizes it. The
//! edges resize it (edges.rs). `title-bar = "system"` in the config gives
//! the desktop's bar back.

use super::*;
use crate::canvas::icons;

/// What can be pressed in the bar.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TitleHit {
    Minimize,
    Maximize,
    Close,
}

/// The bar's state between frames.
#[derive(Default)]
pub struct TitleBar {
    /// Its buttons, where they were drawn.
    hits: Vec<(Rect, TitleHit)>,
    /// The button under the pointer.
    lit: Option<TitleHit>,
    /// What it said when last drawn: drawn again when that changes.
    said: String,
    /// When it was last pressed, for a double click.
    pressed: Option<Instant>,
}

impl App {
    /// The bar's height: none with the desktop's bar, in the quake window,
    /// or full screen.
    pub(super) fn title_h(&self) -> i32 {
        let ours = !crate::quake_mode() && self.window.as_ref().is_some_and(|w| !w.is_decorated() && w.fullscreen().is_none());
        if ours {
            self.fonts.ui.height + self.px(16.0)
        } else {
            0
        }
    }

    pub(super) fn draw_titlebar(&mut self, full: bool) {
        let h = self.title_h();
        if h == 0 || self.workspaces.is_empty() {
            return;
        }
        let maximized = self.window.as_ref().is_some_and(|w| w.is_maximized());
        let ws = self.ws();
        let name = ws.name.clone();
        let folder = self.panes.get(&ws.focus).and_then(|p| p.pty.cwd()).map(|c| persist::shorten(&c)).unwrap_or_default();
        let accent = self.cur_accent;
        let said = format!("{name}\0{folder}\0{accent}\0{maximized}\0{:?}\0{}", self.title.lit, self.canvas.width);
        if !full && said == self.title.said {
            return;
        }
        self.title.said = said;
        let c = self.cur_theme.chrome();
        let px = |v: f32| (v * self.scale).round() as i32;
        let bar = Rect { x: 0, y: 0, w: self.canvas.width, h };
        self.canvas.fill(bar, c.side);
        self.canvas.fill(Rect { x: 0, y: h - 1, w: bar.w, h: 1 }, c.edge);
        // The buttons, from the right: close, maximize, minimize.
        // Small, as Linear's and Vercel's are: 28 by 24, the icons 12.
        let (bw, bh) = (px(28.0), px(24.0).min(h - px(6.0)));
        let icon = px(12.0);
        let mut right = bar.w - px(8.0);
        self.title.hits.clear();
        for (hit, glyph) in [(TitleHit::Close, icons::CLOSE), (TitleHit::Maximize, if maximized { icons::WIN_RESTORE } else { icons::WIN_MAX }), (TitleHit::Minimize, icons::MINUS)] {
            let r = Rect { x: right - bw, y: (h - bh) / 2, w: bw, h: bh };
            let lit = self.title.lit == Some(hit);
            let color = match (hit, lit) {
                (TitleHit::Close, true) => {
                    self.canvas.round_fill(r, px(5.0), 0xE5484D, 46);
                    0xFF6B6F
                }
                (_, true) => {
                    self.canvas.round_fill(r, px(5.0), mix(c.side, c.text, 14), 255);
                    c.text
                }
                _ => c.dim,
            };
            self.canvas.icon_in(glyph, r, icon, color);
            self.title.hits.push((r, hit));
            right = r.x - px(1.0);
        }
        // At the left: the workspace in its color, its name, the folder.
        let uh = self.fonts.ui.height;
        let ty = (h - uh) / 2;
        let mut x = px(14.0);
        let d = px(9.0);
        self.canvas.round_fill(Rect { x, y: (h - d) / 2, w: d, h: d }, px(3.0), accent, 255);
        x += d + px(10.0);
        let room = Rect { x: 0, y: 0, w: (right - px(12.0)).max(0), h };
        x = self.canvas.text_fit(&mut self.fonts.ui, &name, x, ty, (room.w - x).max(0), c.text, room);
        if !folder.is_empty() {
            x += px(10.0);
            self.canvas.text_fit(&mut self.fonts.ui, &folder, x, ty, (room.w - x).max(0), c.dim, room);
        }
        self.damage.push(bar);
    }

    /// The pointer over the bar: its button lit; the arrow, the hand on a
    /// button. True when it is over the bar.
    pub(super) fn title_hover(&mut self, x: i32, y: i32) -> bool {
        let over = y < self.title_h();
        let lit = if over { self.title.hits.iter().find(|(r, _)| r.contains(x, y)).map(|(_, h)| *h) } else { None };
        if lit != self.title.lit {
            self.title.lit = lit;
            self.redraw();
        }
        if over {
            if let Some(w) = &self.window {
                w.set_cursor(if lit.is_some() { CursorIcon::Pointer } else { CursorIcon::Default });
            }
        }
        over
    }

    /// The pointer gone from the window: no button lit.
    pub(super) fn title_leave(&mut self) {
        if self.title.lit.take().is_some() {
            self.redraw();
        }
    }

    /// A press on the bar: its button done, or the window moved (dragged)
    /// or maximized (twice). True when it was on the bar.
    pub(super) fn title_press(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let (x, y) = self.pointer;
        if y >= self.title_h() {
            return false;
        }
        let Some(w) = self.window.clone() else { return true };
        match self.title.hits.iter().find(|(r, _)| r.contains(x, y)).map(|(_, h)| *h) {
            Some(TitleHit::Minimize) => w.set_minimized(true),
            Some(TitleHit::Maximize) => w.set_maximized(!w.is_maximized()),
            Some(TitleHit::Close) => {
                let t = Instant::now();
                crate::debuglog::log("quit: asked");
                self.save_scrollback();
                self.save();
                crate::debuglog::log(format!("quit: saved in {:?}", t.elapsed()));
                event_loop.exit();
            }
            None => {
                let twice = self.title.pressed.is_some_and(|t| t.elapsed() < Duration::from_millis(400));
                if twice {
                    self.title.pressed = None;
                    w.set_maximized(!w.is_maximized());
                } else {
                    self.title.pressed = Some(Instant::now());
                    let _ = w.drag_window();
                }
            }
        }
        self.full = true;
        self.redraw();
        true
    }
}
