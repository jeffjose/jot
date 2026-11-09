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
    /// The sidebar, shown or tucked away: here, where it never moves.
    Sidebar,
    /// The workspace's chip: the switcher, down from it.
    Workspace,
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
        let mark = self.cur_mark();
        let open_down = self.switcher.as_ref().is_some_and(|s| !s.up);
        let said = format!("{name}\0{folder}\0{accent}\0{mark:?}\0{maximized}\0{:?}\0{}\0{open_down}", self.title.lit, self.canvas.width);
        if !full && said == self.title.said {
            return;
        }
        self.title.said = said;
        let c = self.cur_theme.chrome();
        let scale = self.scale;
        let px = move |v: f32| (v * scale).round() as i32;
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
        // At the far left: the sidebar's button, in one place whether the
        // sidebar is shown or not (not in the sidebar, which it hides, nor
        // in a pane's caption once it is hidden).
        let side = Rect { x: px(6.0), y: (h - bh) / 2, w: bw, h: bh };
        let side_lit = self.title.lit == Some(TitleHit::Sidebar);
        if side_lit {
            self.canvas.round_fill(side, px(5.0), mix(c.side, c.text, 14), 255);
        }
        self.canvas.icon_in(icons::SIDEBAR, side, px(14.0), if side_lit { c.text } else { c.dim });
        self.title.hits.push((side, TitleHit::Sidebar));
        // Then the workspace as the foot bar has it at its left: its mark
        // on a square of its color, its name, the folder and a chevron, one
        // thing, which a click opens the switcher down from.
        let uh = self.fonts.ui.height;
        let ty = (h - uh) / 2;
        let room = Rect { x: 0, y: 0, w: (right - px(12.0)).max(0), h };
        let ts = (bh - px(2.0)).max(uh);
        let x0 = side.x + side.w + px(4.0);
        let name_w = self.fonts.ui.width(&name).min((room.w - x0 - ts - px(10.0)).max(0));
        let fx = x0 + px(4.0) + ts + px(10.0) + name_w + px(10.0);
        let folder_w = if folder.is_empty() { 0 } else { self.fonts.ui.width(&folder).min((room.w - fx).max(0)) };
        let cih = px(10.0);
        let cx = if folder.is_empty() { fx - px(10.0) } else { fx + folder_w } + px(8.0);
        let area = Rect { x: x0, y: (h - bh) / 2, w: (cx + cih + px(8.0) - x0).max(0), h: bh };
        let lit = self.title.lit == Some(TitleHit::Workspace);
        let open = self.switcher.as_ref().is_some_and(|s| !s.up);
        if lit || open {
            self.canvas.round_fill(area, px(6.0), accent, if open { 110 } else { 70 });
        }
        self.mark_tile(Rect { x: x0 + px(4.0), y: (h - ts) / 2, w: ts, h: ts }, accent, mark, true);
        let nx = x0 + px(4.0) + ts + px(10.0);
        self.canvas.text_fit(&mut self.fonts.ui, &name, nx, ty, name_w, c.text, room);
        if !folder.is_empty() {
            self.canvas.text_fit(&mut self.fonts.ui, &folder, fx, ty, folder_w, c.dim, room);
        }
        self.canvas.icon_in(icons::CHEVRON_DOWN, Rect { x: cx, y: (h - cih) / 2, w: cih, h: cih }, cih, c.dim);
        self.title.hits.push((area, TitleHit::Workspace));
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
                // The arrow over the window's own buttons too, as every desktop has it.
                w.set_cursor(if lit == Some(TitleHit::Workspace) { CursorIcon::Pointer } else { CursorIcon::Default });
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
            Some(TitleHit::Sidebar) => self.toggle_sidebar(),
            Some(TitleHit::Workspace) => {
                let r = self.title.hits.iter().find(|(_, h)| *h == TitleHit::Workspace).map(|(r, _)| *r).unwrap_or_default();
                self.toggle_switcher(false, (r.x, r.y + r.h));
            }
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
