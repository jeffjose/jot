//! A pane moved to a window of its own (Alt Shift O, or Move to new window in
//! the right-click menu), to put on another screen or beside something
//! else. Its program goes on as it was. Closing the window, or Alt Shift
//! O in it, puts the pane back beside the one you are in.

use super::*;
use winit::window::WindowId;

pub struct Popout {
    pub window: Rc<Window>,
    pub surface: Surface<Rc<Window>, Rc<Window>>,
    pub canvas: Canvas,
    pub pane: PaneId,
    pub pointer: (i32, i32),
    pub selecting: Option<(u16, u16)>,
    pub full: bool,
}

impl App {
    /// The pane you are in, out into a window of its own.
    pub(super) fn pop_out(&mut self, event_loop: &ActiveEventLoop) {
        if self.roam_shown {
            return;
        }
        let id = self.ws().focus;
        // The last pane of a workspace stays: a workspace has one at least.
        if !self.ws_mut().root.remove(id) {
            return;
        }
        self.find = None;
        let mut ids = Vec::new();
        self.ws().root.panes(&mut ids);
        let ws = self.ws_mut();
        ws.zoom = None;
        ws.focus = ids[0];
        self.open_popout_window(id, event_loop);
    }

    /// A window of its own for pane `id`, which is in no workspace.
    pub(super) fn open_popout_window(&mut self, id: PaneId, event_loop: &ActiveEventLoop) {
        let (cols, rows) = self.panes.get(&id).map_or((80, 24), |p| (p.cols, p.rows));
        let size = winit::dpi::PhysicalSize::new((cols as i32 * self.fonts.cell_w + self.px(24.0)) as u32, (rows as i32 * self.fonts.cell_h + self.px(16.0) + self.caption_h()) as u32);
        let title = self.panes.get(&id).map(|p| p.caption.clone()).unwrap_or_default();
        let attrs = window_attributes(&format!("{title} · jot"), size);
        let Ok(window) = event_loop.create_window(attrs) else { return };
        let window = Rc::new(window);
        let Ok(context) = Context::new(window.clone()) else { return };
        let Ok(surface) = Surface::new(&context, window.clone()) else { return };
        let mut popout = Popout { window, surface, canvas: Canvas::new(), pane: id, pointer: (0, 0), selecting: None, full: true };
        let inner = popout.window.inner_size();
        popout.canvas.resize(inner.width as i32, inner.height as i32);
        let _ = popout.surface.resize(NonZeroU32::new(inner.width.max(1)).unwrap(), NonZeroU32::new(inner.height.max(1)).unwrap());
        self.popouts.push(popout);
        self.changed();
    }

    /// A popped-out pane back in its workspace, beside the pane you are in.
    pub(super) fn pop_in(&mut self, wid: WindowId) {
        let Some(i) = self.popouts.iter().position(|p| p.window.id() == wid) else { return };
        let popout = self.popouts.remove(i);
        if self.panes.contains_key(&popout.pane) {
            let ws = self.ws_mut();
            let at = ws.focus;
            ws.root.split(at, popout.pane, Dir::Across);
            ws.focus = popout.pane;
        }
        self.focused_popout = None;
        self.changed();
    }

    /// The popped-out window a pane is in, if it is in one.
    pub(super) fn popout_of(&self, id: PaneId) -> Option<usize> {
        self.popouts.iter().position(|p| p.pane == id)
    }

    /// The panes in windows of their own, laid out to fill them.
    pub(super) fn lay_out_popouts(&mut self) {
        let (cw, ch, cap, pad, top) = (self.fonts.cell_w, self.fonts.cell_h, self.caption_h(), self.px(12.0), self.px(8.0));
        for po in &mut self.popouts {
            let rect = Rect { x: 0, y: 0, w: po.canvas.width, h: po.canvas.height };
            let Some(p) = self.panes.get_mut(&po.pane) else { continue };
            let content = Rect { x: pad, y: cap + top, w: (rect.w - 2 * pad).max(0), h: (rect.h - cap - top).max(0) };
            let cols = (content.w / cw).clamp(1, u16::MAX as i32) as u16;
            let rows = (content.h / ch).clamp(1, u16::MAX as i32) as u16;
            if (cols, rows) != (p.cols, p.rows) {
                p.term.resize(cols, rows, cw as u32, ch as u32);
                p.pty.resize(cols, rows, ((cols as i32 * cw) as u32, (rows as i32 * ch) as u32));
                p.cols = cols;
                p.rows = rows;
                p.lines.clear();
            }
            p.rect = rect;
            p.content = content;
            p.full = true;
            po.full = true;
            po.window.request_redraw();
        }
    }

    /// Draws a popped-out pane on its own window: its canvas is lent to
    /// the drawing the main window uses, and given back.
    pub(super) fn draw_popout(&mut self, wid: WindowId) {
        let Some(i) = self.popouts.iter().position(|p| p.window.id() == wid) else { return };
        let (id, full) = (self.popouts[i].pane, std::mem::take(&mut self.popouts[i].full));
        let focused = self.focused_popout == Some(wid);
        std::mem::swap(&mut self.canvas, &mut self.popouts[i].canvas);
        let damage = std::mem::take(&mut self.damage);
        if full {
            self.canvas.fill(Rect { x: 0, y: 0, w: self.canvas.width, h: self.canvas.height }, self.cur_theme.bg);
            self.damage.push(Rect { x: 0, y: 0, w: self.canvas.width, h: self.canvas.height });
        }
        self.draw_pane(id, 1, full, focused);
        let mine = std::mem::replace(&mut self.damage, damage);
        std::mem::swap(&mut self.canvas, &mut self.popouts[i].canvas);
        let po = &mut self.popouts[i];
        present_frame(&mut po.surface, &po.window, &po.canvas, mine);
    }

    /// What happens in a popped-out window. True when it was one.
    pub(super) fn popout_event(&mut self, wid: WindowId, event: &WindowEvent) -> bool {
        let Some(i) = self.popouts.iter().position(|p| p.window.id() == wid) else { return false };
        let id = self.popouts[i].pane;
        match event {
            WindowEvent::CloseRequested => self.pop_in(wid),
            WindowEvent::RedrawRequested => self.draw_popout(wid),
            WindowEvent::Resized(size) => {
                let po = &mut self.popouts[i];
                let _ = po.surface.resize(NonZeroU32::new(size.width.max(1)).unwrap(), NonZeroU32::new(size.height.max(1)).unwrap());
                po.canvas.resize(size.width as i32, size.height as i32);
                self.lay_out_popouts();
            }
            WindowEvent::Focused(on) => {
                self.focused_popout = if *on { Some(wid) } else { None };
                self.popouts[i].full = true;
                self.popouts[i].window.request_redraw();
                self.full = true;
                self.redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let (x, y) = (position.x as i32, position.y as i32);
                self.popouts[i].pointer = (x, y);
                if let (Some(from), Some(p)) = (self.popouts[i].selecting, self.panes.get_mut(&id)) {
                    let cell = (((x - p.content.x) / self.fonts.cell_w).clamp(0, p.cols as i32 - 1) as u16, ((y - p.content.y) / self.fonts.cell_h).clamp(0, p.rows as i32 - 1) as u16);
                    if cell != from {
                        p.term.select(from, cell);
                        p.full = true;
                        self.popouts[i].window.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let (x, y) = self.popouts[i].pointer;
                let Some(p) = self.panes.get_mut(&id) else { return true };
                let cell = (((x - p.content.x).max(0) / self.fonts.cell_w).min(p.cols as i32 - 1) as u16, ((y - p.content.y).max(0) / self.fonts.cell_h).min(p.rows as i32 - 1) as u16);
                match (state, button) {
                    (ElementState::Pressed, MouseButton::Left) => {
                        p.term.clear_selection();
                        p.full = true;
                        self.popouts[i].selecting = Some(cell);
                    }
                    (ElementState::Released, MouseButton::Left) => {
                        self.popouts[i].selecting = None;
                        if let Some(text) = p.term.selection_text() {
                            self.clip_set(text.clone(), true);
                            self.clip_set(text, false);
                        }
                    }
                    (ElementState::Pressed, MouseButton::Middle) => {
                        if let Some(text) = self.clip_get(true) {
                            if let Some(p) = self.panes.get_mut(&id) {
                                let bytes = p.term.encode_paste(&text);
                                p.pty.write(&bytes);
                            }
                        }
                    }
                    _ => {}
                }
                self.popouts[i].window.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -(y * 3.0) as isize,
                    MouseScrollDelta::PixelDelta(p) => -(p.y / self.fonts.cell_h as f64) as isize,
                };
                if let Some(p) = self.panes.get_mut(&id) {
                    p.term.scroll(lines);
                    p.full = true;
                }
                self.popouts[i].window.request_redraw();
            }
            _ => return false,
        }
        true
    }
}
