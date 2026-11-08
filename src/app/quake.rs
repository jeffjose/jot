//! The quake window, as Tilix's Quake mode has it, and with its settings
//! (Settings › Quake): by default across the top of the monitor the
//! pointer is on, 40% of its height and all its width, above every other
//! window and on every workspace, out of the taskbar. `jot --quake` (a key
//! bound to it) shows it, and hides it again; losing focus does not,
//! unless set to. It is another window on the session: its panes are its
//! own, and run on while it is hidden.

use super::*;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::window::WindowLevel;

/// The quake window: no frame, kept hidden until it is placed.
pub(super) fn attributes() -> winit::window::WindowAttributes {
    use winit::platform::x11::WindowAttributesExtX11;
    let attrs = window_attributes("jot", LogicalSize::new(1200.0, 400.0)).with_decorations(false).with_visible(false);
    WindowAttributesExtX11::with_name(attrs, "jot-quake", "jot-quake")
}

/// How the quake window is to be, from the settings.
pub(super) struct Shape {
    height: u32,
    width: u32,
    bottom: bool,
    align: i32,
    pointer: bool,
    pub all_workspaces: bool,
    pub on_top: bool,
}

impl Shape {
    fn of(c: &Config) -> Shape {
        Shape {
            height: c.quake_height.clamp(10, 100),
            width: c.quake_width.clamp(10, 100),
            bottom: c.quake_position.eq_ignore_ascii_case("bottom"),
            align: match c.quake_align.to_ascii_lowercase().as_str() {
                "left" => -1,
                "right" => 1,
                _ => 0,
            },
            pointer: !c.quake_monitor.eq_ignore_ascii_case("primary"),
            all_workspaces: c.quake_all_workspaces,
            on_top: c.quake_on_top,
        }
    }
}

impl App {
    /// `jot --quake`: shown, or hidden if it shows.
    /// Hidden only when it is the window you are in: shown but under
    /// another (not kept on top), it is brought forward instead, as Tilix
    /// does, so the key never seems to do nothing.
    pub(super) fn quake_toggle(&mut self) {
        if self.quake_shown && self.focused_window {
            self.quake_hide();
        } else {
            self.quake_show();
        }
    }

    /// Its settings as the config has them now: changed in either window,
    /// they hold from the next time it is shown.
    fn quake_settings(&mut self) {
        let c = persist::load_config();
        self.config.quake_height = c.quake_height;
        self.config.quake_width = c.quake_width;
        self.config.quake_position = c.quake_position;
        self.config.quake_align = c.quake_align;
        self.config.quake_monitor = c.quake_monitor;
        self.config.quake_all_workspaces = c.quake_all_workspaces;
        self.config.quake_on_top = c.quake_on_top;
        self.config.quake_hide_on_blur = c.quake_hide_on_blur;
    }

    /// Shown where its settings put it, and given the keys.
    pub(super) fn quake_show(&mut self) {
        let Some(window) = self.window.clone() else { return };
        self.quake_settings();
        let shape = Shape::of(&self.config);
        let x = x11::X::open();
        let xid = x11_id(&window);
        let place = quake_place(&window, x.as_ref(), &shape);
        if let Some((pos, size)) = place {
            let _ = window.request_inner_size(size);
            window.set_outer_position(pos);
        }
        window.set_window_level(if shape.on_top { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal });
        if let (Some(x), Some(id)) = (&x, xid) {
            x.before_map(id, &shape);
        }
        window.set_visible(true);
        if let (Some(x), Some(id)) = (&x, xid) {
            x.wait_mapped(id);
            // Where the window manager put it, put right.
            if let Some((pos, _)) = place {
                window.set_outer_position(pos);
            }
            x.after_map(id, &shape);
        } else {
            window.focus_window();
        }
        self.quake_shown = true;
        self.full = true;
        self.redraw();
    }

    pub(super) fn quake_hide(&mut self) {
        if let Some(w) = &self.window {
            w.set_visible(false);
        }
        self.quake_shown = false;
    }

    /// The quake window lost focus: hidden, if its settings say so.
    pub(super) fn quake_blurred(&mut self) {
        if crate::quake_mode() && self.quake_shown && self.config.quake_hide_on_blur {
            self.quake_hide();
        }
    }
}

/// Where the quake window goes, and how big: the top (or bottom) of the
/// work area of its monitor (the panels left clear), aligned as set.
fn quake_place(window: &Window, x: Option<&x11::X>, shape: &Shape) -> Option<(PhysicalPosition<i32>, PhysicalSize<u32>)> {
    quake_place_area(window, x, shape).map(|(p, s, _)| (p, s))
}

/// As quake_place, and the top and height of the work area it is in.
fn quake_place_area(window: &Window, x: Option<&x11::X>, shape: &Shape) -> Option<(PhysicalPosition<i32>, PhysicalSize<u32>, (i32, i32))> {
    let monitors: Vec<_> = window.available_monitors().collect();
    let pointer = if shape.pointer { x.and_then(|x| x.pointer()) } else { None };
    let monitor = pointer
        .and_then(|(px, py)| {
            monitors.iter().find(|m| {
                let (p, s) = (m.position(), m.size());
                px >= p.x && py >= p.y && px < p.x + s.width as i32 && py < p.y + s.height as i32
            })
        })
        .cloned()
        .or_else(|| window.primary_monitor())
        .or_else(|| monitors.first().cloned())?;
    let (p, s) = (monitor.position(), monitor.size());
    let (mut ax, mut ay, mut aw, mut ah) = (p.x, p.y, s.width as i32, s.height as i32);
    // The work area, where it lies within the monitor.
    if let Some((wx, wy, ww, wh)) = x.and_then(|x| x.workarea()) {
        let (x0, y0) = (ax.max(wx), ay.max(wy));
        let (x1, y1) = ((ax + aw).min(wx + ww), (ay + ah).min(wy + wh));
        if x1 > x0 && y1 > y0 {
            (ax, ay, aw, ah) = (x0, y0, x1 - x0, y1 - y0);
        }
    }
    let w = aw * shape.width as i32 / 100;
    let h = ah * shape.height as i32 / 100;
    let x = match shape.align {
        -1 => ax,
        1 => ax + aw - w,
        _ => ax + (aw - w) / 2,
    };
    let y = if shape.bottom { ay + ah - h } else { ay };
    Some((PhysicalPosition::new(x, y), PhysicalSize::new(w.max(1) as u32, h.max(1) as u32), (ay, ah)))
}

/// The window's X11 id, when it is on X11.
fn x11_id(window: &Window) -> Option<u64> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Xlib(h) => Some(h.window as u64),
        RawWindowHandle::Xcb(h) => Some(h.window.get() as u64),
        _ => None,
    }
}

/// What winit does not say to the window manager: on every workspace, out
/// of the taskbar and pager, and given the keys when shown.
mod x11 {
    use std::ffi::CString;
    use std::os::raw::{c_int, c_long, c_uchar, c_ulong};
    use x11_dl::xlib;
    use super::Shape;

    pub struct X {
        lib: xlib::Xlib,
        dpy: *mut xlib::Display,
    }

    impl Drop for X {
        fn drop(&mut self) {
            unsafe { (self.lib.XCloseDisplay)(self.dpy) };
        }
    }

    impl X {
        pub fn open() -> Option<X> {
            let lib = xlib::Xlib::open().ok()?;
            let dpy = unsafe { (lib.XOpenDisplay)(std::ptr::null()) };
            (!dpy.is_null()).then_some(X { lib, dpy })
        }

        fn root(&self) -> c_ulong {
            unsafe { (self.lib.XDefaultRootWindow)(self.dpy) }
        }

        fn atom(&self, name: &str) -> c_ulong {
            let c = CString::new(name).unwrap_or_default();
            unsafe { (self.lib.XInternAtom)(self.dpy, c.as_ptr(), xlib::False) }
        }

        /// Where the pointer is, on the whole screen.
        pub fn pointer(&self) -> Option<(i32, i32)> {
            let (mut root, mut child) = (0, 0);
            let (mut rx, mut ry, mut wx, mut wy, mut mask) = (0, 0, 0, 0, 0);
            let ok = unsafe { (self.lib.XQueryPointer)(self.dpy, self.root(), &mut root, &mut child, &mut rx, &mut ry, &mut wx, &mut wy, &mut mask) };
            (ok != 0).then_some((rx, ry))
        }

        /// A property of the root window, as numbers.
        fn cardinals(&self, name: &str) -> Vec<c_long> {
            let (mut kind, mut format, mut n, mut after) = (0, 0, 0, 0);
            let mut data: *mut c_uchar = std::ptr::null_mut();
            let got = unsafe { (self.lib.XGetWindowProperty)(self.dpy, self.root(), self.atom(name), 0, 1024, xlib::False, xlib::XA_CARDINAL, &mut kind, &mut format, &mut n, &mut after, &mut data) };
            if got != 0 || data.is_null() {
                return Vec::new();
            }
            let out = if format == 32 { unsafe { std::slice::from_raw_parts(data as *const c_long, n as usize) }.to_vec() } else { Vec::new() };
            unsafe { (self.lib.XFree)(data as *mut _) };
            out
        }

        /// The work area of the workspace shown: the screen less its panels.
        pub fn workarea(&self) -> Option<(i32, i32, i32, i32)> {
            let areas = self.cardinals("_NET_WORKAREA");
            let desk = self.cardinals("_NET_CURRENT_DESKTOP").first().copied().unwrap_or(0).max(0) as usize;
            let a = areas.get(desk * 4..desk * 4 + 4).or_else(|| areas.get(0..4))?;
            Some((a[0] as i32, a[1] as i32, a[2] as i32, a[3] as i32))
        }

        /// Its states as set: out of the taskbar and pager always; over the
        /// others, and on every workspace, if so.
        fn states(&self, shape: &Shape) -> Vec<c_ulong> {
            let mut names = vec!["_NET_WM_STATE_SKIP_TASKBAR", "_NET_WM_STATE_SKIP_PAGER"];
            if shape.on_top {
                names.push("_NET_WM_STATE_ABOVE");
            }
            if shape.all_workspaces {
                names.push("_NET_WM_STATE_STICKY");
            }
            names.into_iter().map(|s| self.atom(s)).collect()
        }

        /// Its workspace: all of them, or the one shown.
        fn desktop(&self, shape: &Shape) -> c_long {
            if shape.all_workspaces {
                0xFFFF_FFFF
            } else {
                self.cardinals("_NET_CURRENT_DESKTOP").first().copied().unwrap_or(0)
            }
        }

        /// Said before it is mapped, which is when the window manager reads
        /// them: its states, and its workspace.
        pub fn before_map(&self, win: u64, shape: &Shape) {
            let states = self.states(shape);
            let desk: [c_long; 1] = [self.desktop(shape)];
            unsafe {
                (self.lib.XChangeProperty)(self.dpy, win as c_ulong, self.atom("_NET_WM_STATE"), xlib::XA_ATOM, 32, xlib::PropModeReplace, states.as_ptr() as *const c_uchar, states.len() as c_int);
                (self.lib.XChangeProperty)(self.dpy, win as c_ulong, self.atom("_NET_WM_DESKTOP"), xlib::XA_CARDINAL, 32, xlib::PropModeReplace, desk.as_ptr() as *const c_uchar, 1);
                (self.lib.XFlush)(self.dpy);
            }
        }

        /// Until the window manager has it on screen, a little while at most.
        pub fn wait_mapped(&self, win: u64) {
            for _ in 0..40 {
                let mut attrs: xlib::XWindowAttributes = unsafe { std::mem::zeroed() };
                let ok = unsafe { (self.lib.XGetWindowAttributes)(self.dpy, win as c_ulong, &mut attrs) };
                if ok != 0 && attrs.map_state == xlib::IsViewable {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }

        fn message(&self, win: u64, kind: &str, data: [c_long; 5]) {
            let mut e: xlib::XEvent = unsafe { std::mem::zeroed() };
            unsafe {
                let m = &mut e.client_message;
                m.type_ = xlib::ClientMessage;
                m.window = win as c_ulong;
                m.message_type = self.atom(kind);
                m.format = 32;
                for (i, d) in data.iter().enumerate() {
                    m.data.set_long(i, *d);
                }
                (self.lib.XSendEvent)(self.dpy, self.root(), xlib::False, xlib::SubstructureRedirectMask | xlib::SubstructureNotifyMask, &mut e);
            }
        }

        /// Said again once it is mapped, for window managers that read them
        /// only then; and the window brought forward and given the keys, as
        /// a pager asks (which focus stealing prevention lets through).
        pub fn after_map(&self, win: u64, shape: &Shape) {
            const ADD: c_long = 1;
            const PAGER: c_long = 2;
            for pair in self.states(shape).chunks(2) {
                self.message(win, "_NET_WM_STATE", [ADD, pair[0] as c_long, pair.get(1).copied().unwrap_or(0) as c_long, PAGER, 0]);
            }
            self.message(win, "_NET_WM_DESKTOP", [self.desktop(shape), PAGER, 0, 0, 0]);
            self.message(win, "_NET_ACTIVE_WINDOW", [PAGER, 0, 0, 0, 0]);
            unsafe { (self.lib.XFlush)(self.dpy) };
        }
    }
}

/// What can be clicked on the quake window's bar.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum QuakeHit {
    Sidebar,
    Workspaces,
    Pane(PaneId),
    Grip,
    Pin,
    Hide,
}

/// The workspace the quake window opens with, which its bar does not name.
pub(super) const QUAKE_WORKSPACE: &str = "quake";

impl App {
    /// The bar along the quake window's open edge: its height (none for
    /// another window).
    pub(super) fn quake_bar_h(&self) -> i32 {
        if crate::quake_mode() {
            self.fonts.ui.height + self.px(10.0)
        } else {
            0
        }
    }

    /// Whether the bar is along the top: the window is at the bottom of
    /// the screen, and opens upward.
    pub(super) fn quake_bar_on_top(&self) -> bool {
        crate::quake_mode() && self.config.quake_position.eq_ignore_ascii_case("bottom")
    }

    fn quake_bar_rect(&self) -> Rect {
        let h = self.quake_bar_h();
        let y = if self.quake_bar_on_top() { 0 } else { self.canvas.height - h };
        Rect { x: 0, y, w: self.canvas.width, h }
    }

    /// The bar: the sidebar's button and the workspaces' (the workspace's
    /// name too, when it is not the quake one), then its panes as tabs; in
    /// the middle the grip, dragged for the height; at the right, kept on
    /// top, and hide.
    pub(super) fn draw_quake_bar(&mut self) {
        self.quake_hits.clear();
        if !crate::quake_mode() {
            return;
        }
        let bar = self.quake_bar_rect();
        let t = self.cur_theme;
        let lift = if t.dark { 0xFFFFFF } else { 0x000000 };
        let (bg, edge) = (mix(t.bg, lift, 16), mix(t.bg, lift, 34));
        let (text, dim, faint) = if t.dark { (0xE4E4E6, 0x8A8C92, 0x55575D) } else { (0x17181B, 0x5C5F66, 0x9A9DA4) };
        self.canvas.fill(bar, bg);
        let line_y = if self.quake_bar_on_top() { bar.y + bar.h - 1 } else { bar.y };
        self.canvas.fill(Rect { y: line_y, h: 1, ..bar }, edge);
        let scale = self.scale;
        let px = move |v: f32| (v * scale).round() as i32;
        let (b, icon, r5) = (bar.h - px(6.0), px(13.0), px(5.0));
        let by = bar.y + (bar.h - b) / 2;
        let ty = bar.y + (bar.h - self.fonts.small.height) / 2;
        let lit = self.quake_lit;
        let mut hits = Vec::new();
        let mut button = |app: &mut App, x: i32, hit: QuakeHit, glyph: char, color: u32| -> i32 {
            let r = Rect { x, y: by, w: b + px(2.0), h: b };
            if lit == Some(hit) {
                app.canvas.round_fill(r, r5, lift, 22);
            }
            app.canvas.icon_in(glyph, r, icon, if lit == Some(hit) { text } else { color });
            hits.push((r, hit));
            r.x + r.w + px(2.0)
        };
        // The middle: the grip.
        let mid = bar.x + bar.w / 2;
        let grip = Rect { x: mid - px(30.0), y: bar.y, w: px(60.0), h: bar.h };
        let gw = px(36.0);
        let gc = if lit == Some(QuakeHit::Grip) || matches!(self.drag, Some(Drag::QuakeGrip)) { dim } else { faint };
        self.canvas.round_fill(Rect { x: mid - gw / 2, y: bar.y + bar.h / 2 - px(1.5), w: gw, h: px(3.0).max(2) }, px(1.5), gc, 255);
        // The right: kept on top (lit while it is), and hide.
        let on_top = self.config.quake_on_top;
        let accent = mix(self.cur_accent, lift, 64);
        let right = bar.x + bar.w - px(6.0) - 2 * (b + px(4.0));
        let x = button(self, right, QuakeHit::Pin, crate::canvas::icons::PIN, if on_top { accent } else { dim });
        let hide = if self.quake_bar_on_top() { crate::canvas::icons::CHEVRON_DOWN } else { crate::canvas::icons::CHEVRON_UP };
        button(self, x, QuakeHit::Hide, hide, dim);
        // The left: the sidebar, the workspaces.
        let mut x = button(self, bar.x + px(6.0), QuakeHit::Sidebar, crate::canvas::icons::SIDEBAR, dim);
        let ws_name = self.ws().name.clone();
        let named = !ws_name.eq_ignore_ascii_case(QUAKE_WORKSPACE);
        {
            let small = &mut self.fonts.small;
            let name_w = if named { small.width(&ws_name) + px(8.0) + px(6.0) } else { 0 };
            let w = px(7.0) + if named { px(6.0) + name_w } else { icon } + px(4.0) + px(9.0) + px(6.0);
            let r = Rect { x, y: by, w, h: b };
            self.canvas.round_fill(r, r5, lift, if lit == Some(QuakeHit::Workspaces) { 30 } else { 18 });
            let mut cx = r.x + px(7.0);
            if named {
                let dot = px(6.0);
                let color = self.cur_accent;
                self.canvas.round_fill(Rect { x: cx, y: bar.y + (bar.h - dot) / 2, w: dot, h: dot }, px(2.0), color, 255);
                cx += dot + px(6.0);
                cx = self.canvas.text(&mut self.fonts.small, &ws_name, cx as f32, ty, text, r) + px(4.0);
            } else {
                self.canvas.icon(crate::canvas::icons::LAYERS, icon, cx, bar.y + (bar.h - icon) / 2, text, r);
                cx += icon + px(4.0);
            }
            let ch = px(9.0);
            self.canvas.icon(crate::canvas::icons::CHEVRON_DOWN, ch, cx, bar.y + (bar.h - ch) / 2, dim, r);
            hits.push((r, QuakeHit::Workspaces));
            x = r.x + r.w + px(4.0);
        }
        // Its panes, as tabs, in the room up to the grip (a lone pane needs
        // none), after the workspace's name when it has one.
        let mut ids = Vec::new();
        self.ws().root.panes(&mut ids);
        if named && ids.len() > 1 {
            x = self.canvas.text(&mut self.fonts.small, "›", (x + px(2.0)) as f32, ty, faint, bar) + px(6.0);
        }
        let focus = self.ws().focus;
        let end = grip.x - px(8.0);
        let tabs: Vec<(PaneId, String)> = ids.iter().filter_map(|id| self.panes.get(id).map(|p| (*id, tab_name(p)))).collect();
        if tabs.len() > 1 && x < end {
            let group = Rect { x, y: by, w: 0, h: b };
            let mut tx = x;
            let mut rects = Vec::new();
            for (n, (id, name)) in tabs.iter().enumerate() {
                let num = format!("{}", n + 1);
                let w = (px(9.0) + self.fonts.small.width(&num) + px(5.0) + self.fonts.small.width(name) + px(9.0)).min(px(160.0));
                if tx + w > end {
                    break;
                }
                rects.push((Rect { x: tx, y: by, w, h: b }, *id, num, name.clone()));
                tx += w;
            }
            let group = Rect { w: tx - x, ..group };
            self.canvas.ring(group, r5, 1, edge);
            for (k, (r, id, num, name)) in rects.into_iter().enumerate() {
                let on = id == focus;
                if on {
                    self.canvas.round_fill(Rect { x: r.x + 1, y: r.y + 1, w: r.w - 2, h: r.h - 2 }, r5 - 1, lift, 34);
                } else if lit == Some(QuakeHit::Pane(id)) {
                    self.canvas.round_fill(Rect { x: r.x + 1, y: r.y + 1, w: r.w - 2, h: r.h - 2 }, r5 - 1, lift, 16);
                }
                if k > 0 {
                    self.canvas.fill(Rect { x: r.x, y: r.y + px(4.0), w: 1, h: r.h - px(8.0) }, edge);
                }
                let nx = self.canvas.text(&mut self.fonts.small, &num, (r.x + px(9.0)) as f32, ty, if on { accent } else { faint }, r) + px(5.0);
                self.canvas.text_fit(&mut self.fonts.small, &name, nx, ty, r.x + r.w - px(9.0) - nx, if on { text } else { dim }, r);
                hits.push((r, QuakeHit::Pane(id)));
            }
        }
        hits.push((grip, QuakeHit::Grip));
        self.quake_hits = hits;
        self.damage.push(bar);
    }

    fn quake_hit_at(&self, x: i32, y: i32) -> Option<QuakeHit> {
        self.quake_hits.iter().find(|(r, _)| r.contains(x, y)).map(|(_, h)| *h)
    }

    /// The pointer over the bar: what it is on lit, and the hand.
    pub(super) fn quake_bar_hover(&mut self, x: i32, y: i32) -> bool {
        if !crate::quake_mode() {
            return false;
        }
        let at = self.quake_hit_at(x, y);
        if at != self.quake_lit {
            self.quake_lit = at;
            self.full = true;
            self.redraw();
        }
        let on_bar = self.quake_bar_rect().contains(x, y);
        if on_bar {
            if let Some(w) = &self.window {
                w.set_cursor(match at {
                    Some(QuakeHit::Grip) => CursorIcon::RowResize,
                    Some(_) => CursorIcon::Pointer,
                    None => CursorIcon::Default,
                });
            }
        }
        on_bar
    }

    /// A click on the bar. True when it was on it.
    pub(super) fn quake_bar_click(&mut self) -> bool {
        if !crate::quake_mode() {
            return false;
        }
        let (x, y) = self.pointer;
        if !self.quake_bar_rect().contains(x, y) {
            return false;
        }
        match self.quake_hit_at(x, y) {
            Some(QuakeHit::Sidebar) => self.toggle_sidebar(),
            Some(QuakeHit::Workspaces) => self.quake_workspaces_menu(),
            Some(QuakeHit::Pane(id)) => {
                self.set_focus(id);
                self.full = true;
                self.redraw();
            }
            Some(QuakeHit::Grip) => self.drag = Some(Drag::QuakeGrip),
            Some(QuakeHit::Pin) => {
                self.config.quake_on_top = !self.config.quake_on_top;
                persist::set_config_value("quake-on-top", if self.config.quake_on_top { "true" } else { "false" });
                if let Some(w) = &self.window {
                    w.set_window_level(if self.config.quake_on_top { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal });
                }
                self.full = true;
                self.redraw();
            }
            Some(QuakeHit::Hide) => self.quake_hide(),
            None => {}
        }
        true
    }

    /// The workspaces to show in the quake window, each with how many panes
    /// it has; and a new one.
    fn quake_workspaces_menu(&mut self) {
        use crate::palette::{Action, Item};
        let mut items: Vec<Option<Item>> = self
            .workspaces
            .iter()
            .enumerate()
            .map(|(i, ws)| {
                let mut ids = Vec::new();
                ws.root.panes(&mut ids);
                let n = ids.len();
                Some(Item { label: ws.name.clone(), hint: format!("{n} pane{}", if n == 1 { "" } else { "s" }), action: Action::GoTo(i) })
            })
            .collect();
        items.push(None);
        items.push(Some(Item { label: "New workspace".into(), hint: "Ctrl N".into(), action: Action::NewWorkspace }));
        self.open_items_menu(items);
    }

    /// The grip being dragged: the window as tall as the pointer makes it.
    pub(super) fn quake_grip_move(&mut self) {
        let Some(window) = self.window.clone() else { return };
        let Some(x) = x11::X::open() else { return };
        let Some((_, gy)) = x.pointer() else { return };
        let shape = Shape::of(&self.config);
        let Some((pos, size, (ay, ah))) = quake_place_area(&window, Some(&x), &shape) else { return };
        let min = (ah / 10).max(self.px(80.0));
        let h = if shape.bottom { ay + ah - gy } else { gy - ay }.clamp(min, ah);
        let _ = window.request_inner_size(PhysicalSize::new(size.width, h as u32));
        if shape.bottom {
            window.set_outer_position(PhysicalPosition::new(pos.x, ay + ah - h));
        }
        self.quake_drag_h = Some((h, ah));
    }

    /// The grip let go: its height kept, as a share of the screen.
    pub(super) fn quake_grip_end(&mut self) {
        self.drag = None;
        if let Some((h, ah)) = self.quake_drag_h.take() {
            let pct = ((h * 100 + ah / 2) / ah.max(1)).clamp(10, 100) as u32;
            self.config.quake_height = pct;
            persist::set_config_value("quake-height", &pct.to_string());
        }
        self.full = true;
        self.redraw();
    }
}

/// A pane's name on a tab: what it runs, or the last part of its title.
fn tab_name(p: &Pane) -> String {
    if let Some(run) = p.shell.running.as_ref().filter(|_| !p.shell.at_prompt) {
        let cmd = run.2.trim();
        if !cmd.is_empty() {
            return cmd.split_whitespace().next().unwrap_or(cmd).to_string();
        }
    }
    let c = p.caption.trim_end_matches('/');
    c.rsplit(['/', ':']).next().filter(|s| !s.is_empty()).unwrap_or(c).to_string()
}

thread_local! {
    /// What the quake bar showed when last drawn.
    static BAR_SEEN: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

impl App {
    /// The quake bar, drawn when the frame is drawn whole, or when what it
    /// shows has changed: the workspace, the pane gone to (keys move it
    /// without a whole frame), its panes and their names, kept on top.
    pub(super) fn quake_bar_frame(&mut self, full: bool) {
        if !crate::quake_mode() {
            return;
        }
        let mut ids = Vec::new();
        self.ws().root.panes(&mut ids);
        let names: Vec<String> = ids.iter().filter_map(|id| self.panes.get(id).map(tab_name)).collect();
        let key = format!("{} {} {:?} {:?} {:?} {}", self.current, self.ws().name, self.ws().focus, ids, names, self.config.quake_on_top);
        let changed = BAR_SEEN.with(|s| *s.borrow() != key);
        if full || changed {
            self.draw_quake_bar();
            BAR_SEEN.with(|s| *s.borrow_mut() = key);
        }
    }
}
