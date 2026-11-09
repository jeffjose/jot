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
    use winit::platform::wayland::WindowAttributesExtWayland;
    use winit::platform::x11::WindowAttributesExtX11;
    let attrs = window_attributes("jot", LogicalSize::new(1200.0, 400.0)).with_decorations(false).with_visible(false);
    let attrs = WindowAttributesExtX11::with_name(attrs, "jot-quake", "jot-quake");
    WindowAttributesExtWayland::with_name(attrs, "jot-quake", "jot-quake")
}

/// How the quake window is to be, from the settings.
pub(crate) struct Shape {
    height: u32,
    /// Its sides' distances in from the screen's edges, in percent of its
    /// width.
    left: f32,
    right: f32,
    bottom: bool,
    pointer: bool,
    pub all_workspaces: bool,
}

impl Shape {
    fn of(c: &Config) -> Shape {
        // As dragged; else as its width and alignment say.
        let width = c.quake_width.clamp(10, 100) as f32;
        let (left, right) = if c.quake_left >= 0.0 && c.quake_right >= 0.0 {
            (c.quake_left, c.quake_right)
        } else {
            let left = match c.quake_align.to_ascii_lowercase().as_str() {
                "left" => 0.0,
                "right" => 100.0 - width,
                _ => (100.0 - width) / 2.0,
            };
            (left, 100.0 - width - left)
        };
        let (left, right) = (left.clamp(0.0, 90.0), right.clamp(0.0, 90.0));
        let right = right.min(90.0 - left);
        Shape {
            height: c.quake_height.clamp(10, 100),
            left,
            right,
            bottom: c.quake_position.eq_ignore_ascii_case("bottom"),
            pointer: !c.quake_monitor.eq_ignore_ascii_case("primary"),
            all_workspaces: c.quake_all_workspaces,
        }
    }

    /// Whether it is narrower than the screen.
    pub fn narrowed(&self) -> bool {
        self.left > 0.05 || self.right > 0.05
    }

    /// Its rectangle as shares of the monitor's work area (`[x, y, w, h]`).
    pub fn rect(&self) -> [f32; 4] {
        let w = ((100.0 - self.left - self.right) / 100.0).clamp(0.1, 1.0);
        let h = (self.height as f32 / 100.0).clamp(0.1, 1.0);
        let x = (self.left / 100.0).clamp(0.0, 1.0 - w);
        let y = if self.bottom { 1.0 - h } else { 0.0 };
        [x, y, w, h]
    }
}

impl App {
    /// `jot --quake`: shown, or hidden if it shows: when it is the window
    /// you are in, or in sight though you clicked elsewhere. Shown but
    /// under another window that is also kept on top, it is brought
    /// forward instead, as Tilix does, so the key never seems to do nothing.
    pub(super) fn quake_toggle(&mut self) {
        self.quake_settings();
        let shape = Shape::of(&self.config);
        if let Some(shown) = wayland::toggle(&shape, self.config.quake_hide_on_blur) {
            self.quake_shown = shown;
            if shown {
                self.full = true;
                self.redraw();
            }
            return;
        }
        let covered = || {
            let w = self.window.as_ref()?;
            Some(x11::X::open()?.covered(x11_id(w)?))
        };
        if self.quake_shown && (self.focused_window || covered() == Some(false)) {
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
        self.config.quake_left = c.quake_left;
        self.config.quake_right = c.quake_right;
        self.config.quake_monitor = c.quake_monitor;
        self.config.quake_all_workspaces = c.quake_all_workspaces;
        self.config.quake_hide_on_blur = c.quake_hide_on_blur;
    }

    /// Shown where its settings put it, and given the keys.
    pub(super) fn quake_show(&mut self) {
        let Some(window) = self.window.clone() else { return };
        self.quake_settings();
        let shape = Shape::of(&self.config);
        if wayland::show(&shape, self.config.quake_hide_on_blur) {
            self.quake_shown = true;
            self.full = true;
            self.redraw();
            return;
        }
        let x = x11::X::open();
        let xid = x11_id(&window);
        let place = quake_place(&window, x.as_ref(), &shape);
        if let Some((pos, size)) = place {
            let _ = window.request_inner_size(size);
            window.set_outer_position(pos);
        }
        // Over every other window, always.
        window.set_window_level(WindowLevel::AlwaysOnTop);
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
        if !wayland::hide() {
            if let Some(w) = &self.window {
                w.set_visible(false);
            }
        }
        self.quake_shown = false;
    }

    /// The quake window lost focus: hidden, if its settings say so.
    pub(super) fn quake_blurred(&mut self) {
        if crate::quake_mode() && self.quake_shown && self.config.quake_hide_on_blur {
            self.quake_hide();
        }
    }

    /// Its settings changed in Settings while the quake window is open:
    /// put where they say now.
    pub(super) fn quake_apply_settings(&mut self) {
        if !crate::quake_mode() || !self.quake_shown {
            return;
        }
        let shape = Shape::of(&self.config);
        if !wayland::place(&shape, self.config.quake_hide_on_blur) {
            if let Some(window) = self.window.clone() {
                let x = x11::X::open();
                if let Some((pos, size, _)) = quake_place_area(&window, x.as_ref(), &shape) {
                    let _ = window.request_inner_size(size);
                    window.set_outer_position(pos);
                }
                if let (Some(x), Some(id)) = (&x, x11_id(&window)) {
                    x.after_map(id, &shape);
                }
            }
        }
        self.full = true;
        self.redraw();
    }
}

/// Where the quake window goes, and how big: the top (or bottom) of the
/// work area of its monitor (the panels left clear), aligned as set.
fn quake_place(window: &Window, x: Option<&x11::X>, shape: &Shape) -> Option<(PhysicalPosition<i32>, PhysicalSize<u32>)> {
    quake_place_area(window, x, shape).map(|(p, s, _)| (p, s))
}

/// As quake_place, and the top and height of the work area it is in.
fn quake_place_area(window: &Window, x: Option<&x11::X>, shape: &Shape) -> Option<(PhysicalPosition<i32>, PhysicalSize<u32>, (i32, i32, i32, i32))> {
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
    let x = ax + (aw as f32 * shape.left / 100.0).round() as i32;
    let w = (aw as f32 * (100.0 - shape.left - shape.right) / 100.0).round() as i32;
    let h = ah * shape.height as i32 / 100;
    let y = if shape.bottom { ay + ah - h } else { ay };
    Some((PhysicalPosition::new(x, y), PhysicalSize::new(w.max(1) as u32, h.max(1) as u32), (ax, ay, aw, ah)))
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

        /// Its states: over every other window and out of the taskbar and
        /// pager always; on every workspace, if so.
        fn states(&self, shape: &Shape) -> Vec<c_ulong> {
            let mut names = vec!["_NET_WM_STATE_SKIP_TASKBAR", "_NET_WM_STATE_SKIP_PAGER"];
            names.push("_NET_WM_STATE_ABOVE");
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

        /// A property of window `win`, as numbers (windows, atoms or
        /// cardinals, as `kind` says).
        fn longs(&self, win: c_ulong, name: &str, kind: c_ulong) -> Vec<c_ulong> {
            let (mut got_kind, mut format, mut n, mut after) = (0, 0, 0, 0);
            let mut data: *mut c_uchar = std::ptr::null_mut();
            let got = unsafe { (self.lib.XGetWindowProperty)(self.dpy, win, self.atom(name), 0, 4096, xlib::False, kind, &mut got_kind, &mut format, &mut n, &mut after, &mut data) };
            if got != 0 || data.is_null() {
                return Vec::new();
            }
            let out = if format == 32 { unsafe { std::slice::from_raw_parts(data as *const c_ulong, n as usize) }.to_vec() } else { Vec::new() };
            unsafe { (self.lib.XFree)(data as *mut _) };
            out
        }

        /// Where window `win` is on the screen, and how big.
        fn rect(&self, win: c_ulong) -> Option<(i32, i32, i32, i32)> {
            let mut a: xlib::XWindowAttributes = unsafe { std::mem::zeroed() };
            if unsafe { (self.lib.XGetWindowAttributes)(self.dpy, win, &mut a) } == 0 || a.map_state != xlib::IsViewable {
                return None;
            }
            let (mut x, mut y, mut child) = (0, 0, 0);
            unsafe { (self.lib.XTranslateCoordinates)(self.dpy, win, self.root(), 0, 0, &mut x, &mut y, &mut child) };
            Some((x, y, a.width, a.height))
        }

        /// Whether a window over `win` hides much of it: one above it in
        /// the window manager's stacking, on the workspace shown and not
        /// minimized, over a quarter of it or more. Unknown (no such list):
        /// not covered.
        pub fn covered(&self, win: u64) -> bool {
            let win = win as c_ulong;
            let stack = self.longs(self.root(), "_NET_CLIENT_LIST_STACKING", xlib::XA_WINDOW);
            let Some(at) = stack.iter().position(|w| *w == win) else { return false };
            let Some((x, y, w, h)) = self.rect(win) else { return false };
            let desk = self.cardinals("_NET_CURRENT_DESKTOP").first().copied().unwrap_or(0) as c_ulong;
            let hidden = self.atom("_NET_WM_STATE_HIDDEN");
            stack[at + 1..].iter().any(|&o| {
                let on = self.longs(o, "_NET_WM_DESKTOP", xlib::XA_CARDINAL).first().is_none_or(|d| *d == desk || *d as u32 == u32::MAX);
                if !on || self.longs(o, "_NET_WM_STATE", xlib::XA_ATOM).contains(&hidden) {
                    return false;
                }
                let Some((ox, oy, ow, oh)) = self.rect(o) else { return false };
                let iw = (x + w).min(ox + ow) - x.max(ox);
                let ih = (y + h).min(oy + oh) - y.max(oy);
                iw > 0 && ih > 0 && (iw as i64 * ih as i64) * 4 >= w as i64 * h as i64
            })
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
    /// A terminal of the machine listed, as its sidebar row.
    Term(sidebar::SideHit),
    /// The machine whose terminals are listed: the others, to pick from.
    Machine,
    /// The bar's empty stretch, dragged for the height.
    Grip,
    Hide,
    /// Make it full width again (there while it is narrower).
    Full,
    /// Its default height again (there while it is not).
    Height,
}

/// jot's own color, its icon's: the quake window's square, which stands
/// for jot rather than a workspace.
const JOT_COLOR: u32 = 0xE54D2E;

/// jot's icon `size` pixels square, 0xAARRGGBB: each pixel the mean of the
/// part of the 256-pixel icon it stands for, its colors weighed by how
/// opaque they are, so the edges stay clean; kept once made.
fn app_mark(size: u32) -> std::rc::Rc<Vec<u32>> {
    thread_local! {
        static MADE: std::cell::RefCell<Option<(u32, std::rc::Rc<Vec<u32>>)>> = const { std::cell::RefCell::new(None) };
    }
    if let Some(m) = MADE.with(|m| m.borrow().as_ref().filter(|(s, _)| *s == size).map(|(_, p)| p.clone())) {
        return m;
    }
    static PNG: &[u8] = include_bytes!("../../assets/jot.png");
    let out = (|| {
        let mut reader = png::Decoder::new(std::io::Cursor::new(PNG)).read_info().ok()?;
        let mut buf = vec![0; reader.output_buffer_size()?];
        let info = reader.next_frame(&mut buf).ok()?;
        if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
            return None;
        }
        let (w, h, s) = (info.width as usize, info.height as usize, size as usize);
        let mut out = vec![0u32; s * s];
        for y in 0..s {
            let (y0, y1) = (y * h / s, ((y + 1) * h / s).max(y * h / s + 1));
            for x in 0..s {
                let (x0, x1) = (x * w / s, ((x + 1) * w / s).max(x * w / s + 1));
                let mut sum = [0u64; 4];
                for sy in y0..y1 {
                    for sx in x0..x1 {
                        let i = (sy * w + sx) * 4;
                        let a = buf[i + 3] as u64;
                        sum[0] += buf[i] as u64 * a;
                        sum[1] += buf[i + 1] as u64 * a;
                        sum[2] += buf[i + 2] as u64 * a;
                        sum[3] += a;
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)) as u64;
                let a = sum[3] / n;
                if sum[3] > 0 {
                    let ch = |v: u64| (v / sum[3]) as u32;
                    out[y * s + x] = (a as u32) << 24 | ch(sum[0]) << 16 | ch(sum[1]) << 8 | ch(sum[2]);
                }
            }
        }
        Some(out)
    })()
    .unwrap_or_else(|| vec![0; (size * size) as usize]);
    let out = std::rc::Rc::new(out);
    MADE.with(|m| *m.borrow_mut() = Some((size, out.clone())));
    out
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

    /// The bar, as the main window's along its foot, its look the same: at
    /// the left jot's own mark (the quake window is jot's, no workspace's);
    /// then every terminal of the machine its workspace is on, those in its
    /// panes numbered, the one you are in lit, any clicked shown in the
    /// pane you are in; at the right that machine, a button to go to
    /// another's workspace, as the main window's does, and hide. Its
    /// empty stretch is dragged for the window's height.
    pub(super) fn draw_quake_bar(&mut self) {
        self.quake_hits.clear();
        if !crate::quake_mode() {
            return;
        }
        let bar = self.quake_bar_rect();
        let c = self.cur_theme.chrome();
        let lift = if self.cur_theme.dark { 0xFFFFFF } else { 0x000000 };
        self.canvas.fill(bar, c.side);
        let line_y = if self.quake_bar_on_top() { bar.y + bar.h - 1 } else { bar.y };
        self.canvas.fill(Rect { y: line_y, h: 1, ..bar }, c.edge);
        let scale = self.scale;
        let px = move |v: f32| (v * scale).round() as i32;
        let h = bar.h;
        let ty = bar.y + (h - self.fonts.ui.height) / 2;
        let bh = (h - px(5.0)).max(px(16.0));
        let by = bar.y + (h - bh) / 2;
        let lit = self.quake_lit;
        let mut hits = Vec::new();
        // The square: jot's mark on jot's color.
        let tile = Rect { x: bar.x, y: bar.y, w: h, h };
        self.canvas.fill(tile, JOT_COLOR);
        let m = (h as f32 * 0.9).round() as i32;
        let mark = app_mark(m.max(1) as u32);
        self.canvas.picture(&mark, m as u32, (0, 0, m as u32, m as u32), Rect { x: tile.x + (h - m) / 2, y: tile.y + (h - m) / 2, w: m, h: m }, tile);
        // The right: hide; the machine, a button.
        let hw = bh + px(4.0);
        let hide = Rect { x: bar.x + bar.w - px(4.0) - hw, y: by, w: hw, h: bh };
        if lit == Some(QuakeHit::Hide) {
            self.canvas.round_fill(hide, px(5.0), lift, 30);
        }
        let chevron = if self.quake_bar_on_top() { crate::canvas::icons::CHEVRON_DOWN } else { crate::canvas::icons::CHEVRON_UP };
        self.canvas.icon_in(chevron, hide, px(12.0), if lit == Some(QuakeHit::Hide) { c.text } else { c.dim });
        hits.push((hide, QuakeHit::Hide));
        // Narrower than the screen: a button to make it full width again.
        let mut left_of = hide.x;
        if Shape::of(&self.config).narrowed() {
            let full = Rect { x: hide.x - px(4.0) - hw, y: by, w: hw, h: bh };
            let on = lit == Some(QuakeHit::Full);
            if on {
                self.canvas.round_fill(full, px(5.0), lift, 30);
            }
            // ⟷: arrows to both sides.
            let (cx, cy, half) = (full.x + full.w / 2, full.y + full.h / 2, px(6.0));
            let color = if on { c.text } else { c.dim };
            self.canvas.fill(Rect { x: cx - half, y: cy, w: 2 * half, h: px(1.5).max(1) }, color);
            for (sx, d) in [(cx - half, 1), (cx + half, -1)] {
                for i in 0..px(3.0) {
                    self.canvas.fill(Rect { x: sx + d * i, y: cy - i, w: 1, h: 2 * i + 1 }, color);
                }
            }
            hits.push((full, QuakeHit::Full));
            left_of = full.x;
        }
        // Not its default height: a button to make it so again.
        if self.config.quake_height != persist::default_quake_height() {
            let tall = Rect { x: left_of - px(4.0) - hw, y: by, w: hw, h: bh };
            let on = lit == Some(QuakeHit::Height);
            if on {
                self.canvas.round_fill(tall, px(5.0), lift, 30);
            }
            // ↕: arrows up and down.
            let (cx, cy, half) = (tall.x + tall.w / 2, tall.y + tall.h / 2, px(6.0));
            let color = if on { c.text } else { c.dim };
            self.canvas.fill(Rect { x: cx, y: cy - half, w: px(1.5).max(1), h: 2 * half }, color);
            for (sy, d) in [(cy - half, 1), (cy + half, -1)] {
                for i in 0..px(3.0) {
                    self.canvas.fill(Rect { x: cx - i, y: sy + d * i, w: 2 * i + 1, h: 1 }, color);
                }
            }
            hits.push((tall, QuakeHit::Height));
            left_of = tall.x;
        }
        let k = self.foot_machine();
        let machine = self.machine_name(k);
        let ih = px(10.0);
        let bw = px(8.0) + self.fonts.ui.width(&machine) + px(6.0) + ih + px(7.0);
        let button = Rect { x: left_of - px(6.0) - bw, y: by, w: bw, h: bh };
        let on = lit == Some(QuakeHit::Machine);
        self.canvas.round_fill(button, px(5.0), lift, if on { 30 } else { 14 });
        self.canvas.text(&mut self.fonts.ui, &machine, (button.x + px(8.0)) as f32, ty, if on { c.text } else { mix(c.dim, c.text, 128) }, bar);
        let ix = button.x + button.w - px(7.0) - ih;
        let open = if self.quake_bar_on_top() { crate::canvas::icons::CHEVRON_DOWN } else { crate::canvas::icons::CHEVRON_UP };
        self.canvas.icon_in(open, Rect { x: ix, y: button.y + (button.h - ih) / 2, w: ih, h: ih }, ih, c.dim);
        hits.push((button, QuakeHit::Machine));
        // The machine's terminals, in the order its session made them: in a
        // pane of this window its number, the one you are in lit.
        let end = button.x - px(12.0);
        let focus = self.focused().map(crate::mux::base_id);
        let mut x = tile.x + tile.w + px(6.0);
        for t in self.side_terminals().into_iter().filter(|t| t.k == k) {
            let num = t.pane.filter(|_| t.here).and_then(|p| self.term_number(p)).map(|n| n.to_string());
            let label = t.label.clone();
            let nw = num.as_ref().map_or(px(5.0), |n| self.fonts.small.width(n) + px(8.0));
            let editing = self.inline.as_ref().is_some_and(|i| i.target == super::inline::Target::Terminal(t.k, t.sid));
            let w = (px(8.0) + nw + px(6.0) + self.fonts.ui.width(&label) + px(9.0)).min(px(180.0)).max(if editing { px(200.0) } else { 0 });
            if x + w > end {
                break;
            }
            let r = Rect { x, y: by, w, h: bh };
            let hit = match t.pane {
                Some(p) => QuakeHit::Term(sidebar::SideHit::Pane(p)),
                None => QuakeHit::Term(sidebar::SideHit::Free(t.k, t.sid)),
            };
            let current = t.here && t.pane.map(crate::mux::base_id) == focus;
            if current {
                self.canvas.round_fill(r, px(5.0), lift, 34);
            } else if lit == Some(hit) {
                self.canvas.round_fill(r, px(5.0), lift, 16);
            }
            let mut cx = r.x + px(8.0);
            match &num {
                // Its number in a small outlined box, as the sidebar has it.
                Some(n) => {
                    let sh = self.fonts.small.height;
                    let nb = Rect { x: cx, y: bar.y + (h - sh - px(2.0)) / 2, w: nw - px(2.0), h: sh + px(2.0) };
                    self.canvas.ring(nb, px(3.0), 1, c.edge);
                    let sty = nb.y + (nb.h - sh) / 2;
                    self.canvas.text(&mut self.fonts.small, n, (nb.x + px(3.0)) as f32, sty, c.dim, r);
                }
                // In no pane here: a quiet dot, as a sidebar row has.
                None => {
                    let d = 2.5 * scale;
                    self.canvas.disc(cx as f32 + d, bar.y as f32 + h as f32 / 2.0, d, c.faint);
                }
            }
            cx += nw + px(6.0);
            let color = if current { c.text } else if t.here { mix(c.dim, c.text, 160) } else { c.dim };
            if self.draw_inline(super::inline::Target::Terminal(t.k, t.sid), cx, ty, (r.x + r.w - px(9.0) - cx).max(0), false, bar).is_none() {
                self.canvas.text_fit(&mut self.fonts.ui, &label, cx, ty, r.x + r.w - px(9.0) - cx, color, r);
            }
            hits.push((r, hit));
            x += w + px(2.0);
        }
        // What is left: dragged for the height.
        if end > x {
            hits.push((Rect { x, y: bar.y, w: end - x, h }, QuakeHit::Grip));
        }
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
            self.quake_dirty = true;
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
            // As its row in the sidebar: shown in the pane you are in, or
            // gone to when a pane here shows it.
            Some(QuakeHit::Term(hit)) => self.side_row_click(hit),
            Some(QuakeHit::Machine) => self.quake_machine_menu(),
            Some(QuakeHit::Grip) => self.drag = Some(Drag::QuakeGrip),
            Some(QuakeHit::Hide) => self.quake_hide(),
            Some(QuakeHit::Full) => self.quake_full_width(),
            Some(QuakeHit::Height) => self.quake_reset_height(),
            None => {}
        }
        true
    }

    /// The machines, from the bar's machine: another's terminals listed.
    fn quake_machine_menu(&mut self) {
        let Some(&(button, _)) = self.quake_hits.iter().find(|(_, h)| *h == QuakeHit::Machine) else { return };
        self.machine_menu_at(button, !self.quake_bar_on_top());
    }

    /// The grip being dragged: the window as tall as the pointer makes it.
    pub(super) fn quake_grip_move(&mut self) {
        let shape = Shape::of(&self.config);
        if let Some(((_, gy), (ax, ay, aw, ah))) = wayland::pointer_and_area(&shape, self.config.quake_hide_on_blur) {
            let min = (ah / 10).max(80);
            let h = if shape.bottom { ay + ah - gy } else { gy - ay }.clamp(min, ah);
            let x = ax + (aw as f32 * shape.left / 100.0).round() as i32;
            let w = (aw as f32 * (100.0 - shape.left - shape.right) / 100.0).round() as i32;
            let y = if shape.bottom { ay + ah - h } else { ay };
            wayland::place_pixels(x, y, w.max(1), h, &shape, self.config.quake_hide_on_blur);
            self.quake_drag_h = Some((h, ah));
            return;
        }
        let Some(window) = self.window.clone() else { return };
        let Some(x) = x11::X::open() else { return };
        let Some((_, gy)) = x.pointer() else { return };
        let Some((pos, size, (_, ay, _, ah))) = quake_place_area(&window, Some(&x), &shape) else { return };
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
            wayland::place(&Shape::of(&self.config), self.config.quake_hide_on_blur);
        }
        self.full = true;
        self.redraw();
    }
}


thread_local! {
    /// What the quake bar showed when last drawn.
    static BAR_SEEN: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

impl App {
    /// The quake bar, drawn when the frame is drawn whole, or when what it
    /// shows has changed: the workspace, the pane gone to (keys move it
    /// without a whole frame), its panes and their names.
    pub(super) fn quake_bar_frame(&mut self, full: bool) {
        if !crate::quake_mode() {
            return;
        }
        let mut ids = Vec::new();
        self.ws().root.panes(&mut ids);
        let names: Vec<String> = ids.iter().filter_map(|id| self.panes.get(id).map(|_| self.terminal_label(*id))).collect();
        let key = format!("{} {} {:?} {:?} {:?}", self.current, self.ws().name, self.ws().focus, ids, names);
        let changed = BAR_SEEN.with(|s| *s.borrow() != key);
        if full || changed {
            self.draw_quake_bar();
            BAR_SEEN.with(|s| *s.borrow_mut() = key);
        }
    }
}

impl App {
    /// A right click on the quake window's bar: on a terminal, what can be
    /// done with it (as from its sidebar row); elsewhere on the bar,
    /// nothing (not the pane's menu, which is not what was clicked).
    pub(super) fn quake_bar_menu(&mut self) -> bool {
        if !crate::quake_mode() {
            return false;
        }
        let (x, y) = self.pointer;
        if !self.quake_bar_rect().contains(x, y) {
            return false;
        }
        match self.quake_hit_at(x, y) {
            Some(QuakeHit::Term(hit)) => self.terminal_menu(hit),
            // Elsewhere on it: its size and place, back to how they were made.
            _ => {
                use crate::palette::{Action, Item};
                let c = |label: &str, hint: &str, action| Some(Item { label: label.into(), hint: hint.into(), action });
                let shape = Shape::of(&self.config);
                let mut items = Vec::new();
                if shape.narrowed() {
                    items.push(c("Full width", "double-click a side", Action::QuakeFullWidth));
                }
                if (shape.left - shape.right).abs() > 0.5 {
                    items.push(c("Centered", "", Action::QuakeCenter));
                }
                if self.config.quake_height != persist::default_quake_height() {
                    items.push(c("Reset height", "", Action::QuakeResetHeight));
                }
                if !items.is_empty() {
                    items.push(None);
                }
                items.push(c("Hide", "jot --quake", Action::QuakeHide));
                self.open_items_menu(items);
            }
        }
        // Up from the bar along the foot, down from one along the top.
        let (row, pad, on_top) = (self.fonts.ui.height + self.px(8.0), self.px(12.0), self.quake_bar_on_top());
        let below = y + self.px(8.0);
        if let Some(m) = &mut self.menu {
            let h = m.items.len() as i32 * row + pad;
            m.at = if on_top { (x, below) } else { (x, (y - h).max(0)) };
        }
        true
    }

    /// The quake window in the middle of the screen, its width kept.
    pub(super) fn quake_center(&mut self) {
        let shape = Shape::of(&self.config);
        let side = (shape.left + shape.right) / 2.0;
        self.config.quake_left = side;
        self.config.quake_right = side;
        self.quake_sides_keep();
    }

    /// The quake window its default height again.
    pub(super) fn quake_reset_height(&mut self) {
        self.config.quake_height = persist::default_quake_height();
        persist::unset_config_value("quake-height");
        self.quake_sides_keep();
    }
}

/// How near the screen's edge a side dragged snaps to it.
const SNAP: i32 = 16;

impl App {
    /// The side of the quake window the point is at (true the left), within
    /// reach of its edge, above its bar.
    fn quake_side_at(&self, x: i32, y: i32) -> Option<bool> {
        if !crate::quake_mode() || self.quake_bar_rect().contains(x, y) {
            return None;
        }
        let reach = self.px(6.0);
        if x < reach {
            Some(true)
        } else if x >= self.canvas.width - reach {
            Some(false)
        } else {
            None
        }
    }

    /// The pointer near a side: the resize arrows, and a handle lit there.
    /// Whether it is.
    pub(super) fn quake_side_hover(&mut self, x: i32, y: i32) -> bool {
        let at = if matches!(self.drag, Some(Drag::QuakeSide(_))) { self.quake_side_lit } else { self.quake_side_at(x, y) };
        if at != self.quake_side_lit {
            self.quake_side_lit = at;
            self.full = true;
            self.redraw();
        }
        if at.is_some() {
            if let Some(w) = &self.window {
                w.set_cursor(CursorIcon::EwResize);
            }
        }
        at.is_some()
    }

    /// A press at a side: held, to drag; pressed twice, that side back to
    /// the screen's edge.
    pub(super) fn quake_side_press(&mut self) -> bool {
        let (x, y) = self.pointer;
        let Some(left) = self.quake_side_at(x, y) else { return false };
        let again = self.quake_side_click.is_some_and(|(l, t)| l == left && t.elapsed() < Duration::from_millis(400));
        self.quake_side_click = Some((left, Instant::now()));
        if again {
            self.quake_side_click = None;
            if left {
                self.config.quake_left = 0.0;
            } else {
                self.config.quake_right = 0.0;
            }
            self.quake_sides_keep();
            return true;
        }
        self.drag = Some(Drag::QuakeSide(left));
        true
    }

    /// A side dragged: to the pointer, wherever on the screen; within reach
    /// of the screen's edge, to it. The other side stays.
    pub(super) fn quake_side_move(&mut self) {
        let Some(Drag::QuakeSide(left)) = self.drag else { return };
        let Some(window) = self.window.clone() else { return };
        let shape = Shape::of(&self.config);
        let mut x = None;
        let Some((gx, ax, aw)) = (if let Some(((gx, _), (ax, _, aw, _))) = wayland::pointer_and_area(&shape, self.config.quake_hide_on_blur) {
            Some((gx, ax, aw))
        } else {
            x = x11::X::open();
            (|| {
                let x = x.as_ref()?;
                let (gx, _) = x.pointer()?;
                let (_, _, (ax, _, aw, _)) = quake_place_area(&window, Some(x), &shape)?;
                Some((gx, ax, aw))
            })()
        }) else {
            return;
        };
        let aw = aw.max(1);
        let min = (self.px(320.0) as f32 / aw as f32 * 100.0).min(90.0);
        let pct = |px: i32| px as f32 / aw as f32 * 100.0;
        if left {
            let gap = if gx - ax < SNAP { 0 } else { gx - ax };
            self.config.quake_left = pct(gap).clamp(0.0, 100.0 - shape.right - min);
            self.config.quake_right = shape.right;
        } else {
            let gap = if ax + aw - gx < SNAP { 0 } else { ax + aw - gx };
            self.config.quake_right = pct(gap).clamp(0.0, 100.0 - shape.left - min);
            self.config.quake_left = shape.left;
        }
        let shape = Shape::of(&self.config);
        self.quake_readout = Some(if shape.narrowed() { format!("from {:.0}%  ·  {:.0}% wide", shape.left, 100.0 - shape.left - shape.right) } else { "full width".to_string() });
        if !wayland::place(&shape, self.config.quake_hide_on_blur) {
            if let Some((pos, size, _)) = quake_place_area(&window, x.as_ref(), &shape) {
                let _ = window.request_inner_size(size);
                window.set_outer_position(pos);
            }
        }
        self.full = true;
        self.redraw();
    }

    /// A side let go: where both are kept.
    pub(super) fn quake_side_end(&mut self) {
        self.drag = None;
        self.quake_readout = None;
        self.quake_sides_keep();
    }

    /// The quake window full width again (its bar's button).
    pub(super) fn quake_full_width(&mut self) {
        self.config.quake_left = 0.0;
        self.config.quake_right = 0.0;
        self.quake_sides_keep();
    }

    /// Its sides as they are now kept, for the next time it is shown, and
    /// the window put where they say.
    fn quake_sides_keep(&mut self) {
        let shape = Shape::of(&self.config);
        self.config.quake_left = shape.left;
        self.config.quake_right = shape.right;
        persist::set_config_value("quake-left", &format!("{:.2}", shape.left));
        persist::set_config_value("quake-right", &format!("{:.2}", shape.right));
        if !wayland::place(&shape, self.config.quake_hide_on_blur) {
            if let Some(window) = self.window.clone() {
                let x = x11::X::open();
                if let Some((pos, size, _)) = quake_place_area(&window, x.as_ref(), &shape) {
                    let _ = window.request_inner_size(size);
                    window.set_outer_position(pos);
                }
            }
        }
        self.full = true;
        self.redraw();
    }

    /// Over the quake window's text, after it is drawn: the handle at the
    /// side the pointer is at (or held), and while one is dragged, where it
    /// is, at the top.
    pub(super) fn draw_quake_sides(&mut self) {
        if !crate::quake_mode() {
            return;
        }
        let accent = 0x4A9EFF;
        if let Some(left) = self.quake_side_lit {
            let (w, h) = (self.px(4.0), (self.canvas.height / 3).max(self.px(40.0)));
            let held = matches!(self.drag, Some(Drag::QuakeSide(_)));
            let x = if left { self.px(1.0) } else { self.canvas.width - w - self.px(1.0) };
            let r = Rect { x, y: (self.canvas.height - self.quake_bar_h() - h) / 2, w, h };
            self.canvas.round_fill(r, w / 2, if held { accent } else { 0xFFFFFF }, if held { 255 } else { 70 });
            self.damage.push(r);
        }
        if let Some(text) = self.quake_readout.clone() {
            let c = self.cur_theme.chrome();
            let tw = self.fonts.ui.width(&text);
            let pad = self.px(10.0);
            let r = Rect { x: (self.canvas.width - tw) / 2 - pad, y: self.px(10.0), w: tw + 2 * pad, h: self.fonts.ui.height + self.px(10.0) };
            self.canvas.round_fill(r, self.px(6.0), c.panel, 255);
            self.canvas.ring(r, self.px(6.0), 1, accent);
            let ty = r.y + self.px(5.0);
            self.canvas.text(&mut self.fonts.ui, &text, (r.x + pad) as f32, ty, c.text, r);
            self.damage.push(r);
        }
    }
}

impl App {
    /// Its width or alignment set in Settings: the sides as dragged give
    /// way to them.
    pub(super) fn quake_sides_unset(&mut self) {
        self.config.quake_left = persist::unset_side();
        self.config.quake_right = persist::unset_side();
        persist::unset_config_value("quake-left");
        persist::unset_config_value("quake-right");
    }
}

/// On GNOME Wayland, winit cannot move, hide, raise, stick, or query
/// stacking and pointer coordinates for a window; jot's small GNOME Shell
/// extension (`jot@jeffjose.github.io`, installed by `jot setup`) does that
/// in the compositor and answers over `$XDG_RUNTIME_DIR/jot-shell.sock`
/// (with D-Bus `/io/github/jeffjose/Jot` as fallback).
pub(crate) mod wayland {
    use super::Shape;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Duration;

    pub const EXTENSION_UUID: &str = "jot@jeffjose.github.io";
    pub const VERSION: u32 = 1;

    const BODY: &str = include_str!("../../assets/extension.js");

    const INTERFACE: &str = r#"<node>
  <interface name="io.github.jeffjose.Jot">
    <method name="Quake">
      <arg type="s" direction="in" name="want"/>
      <arg type="s" direction="out" name="state"/>
    </method>
  </interface>
</node>"#;

    #[derive(Debug, Clone, PartialEq)]
    pub struct Reply {
        pub state: String,
        pub focused: bool,
        pub covered: bool,
        pub pointer: Option<(i32, i32)>,
        pub area: Option<(i32, i32, i32, i32)>,
        pub frame: Option<(i32, i32, i32, i32)>,
    }

    impl Reply {
        pub fn parse(raw: &str) -> Option<Self> {
            let state = str_field(raw, "state")?;
            let focused = bool_field(raw, "focused").unwrap_or(false);
            let covered = bool_field(raw, "covered").unwrap_or(false);
            let pointer = ints_field(raw, "pointer").and_then(|v| Some((*v.first()?, *v.get(1)?)));
            let area = ints_field(raw, "area").and_then(|v| Some((*v.first()?, *v.get(1)?, *v.get(2)?, *v.get(3)?)));
            let frame = ints_field(raw, "frame").and_then(|v| Some((*v.first()?, *v.get(1)?, *v.get(2)?, *v.get(3)?)));
            Some(Self { state, focused, covered, pointer, area, frame })
        }
    }

    fn str_field(raw: &str, key: &str) -> Option<String> {
        let pat = format!("\"{key}\":\"");
        let rest = raw.split_once(&pat)?.1;
        Some(rest.split_once('"')?.0.to_string())
    }

    fn bool_field(raw: &str, key: &str) -> Option<bool> {
        let pat = format!("\"{key}\":");
        let rest = raw.split_once(&pat)?.1.trim_start();
        if rest.starts_with("true") {
            Some(true)
        } else if rest.starts_with("false") {
            Some(false)
        } else {
            None
        }
    }

    fn ints_field(raw: &str, key: &str) -> Option<Vec<i32>> {
        let pat = format!("\"{key}\":[");
        let rest = raw.split_once(&pat)?.1;
        let inside = rest.split_once(']')?.0;
        inside.split(',').map(|s| s.trim().parse::<i32>().ok()).collect()
    }

    pub fn active() -> bool {
        std::env::var_os("WAYLAND_DISPLAY").is_some()
    }

    fn socket_path() -> PathBuf {
        match std::env::var_os("XDG_RUNTIME_DIR") {
            Some(dir) => PathBuf::from(dir).join("jot-shell.sock"),
            None => {
                let user = std::env::var("USER").unwrap_or_else(|_| unsafe { libc::getuid() }.to_string());
                PathBuf::from(format!("/tmp/jot-{user}-shell.sock"))
            }
        }
    }

    fn json_str(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for ch in s.chars() {
            match ch {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    fn shape_fields(shape: &Shape, blur: bool) -> String {
        let [rx, ry, rw, rh] = shape.rect();
        format!(
            "\"rect\":[{rx:.4},{ry:.4},{rw:.4},{rh:.4}],\"pointer\":{},\"all_workspaces\":{},\"blur\":{}",
            shape.pointer, shape.all_workspaces, blur
        )
    }

    fn ask(json: &str) -> Option<Reply> {
        if let Ok(mut stream) = UnixStream::connect(socket_path()) {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
            if stream.write_all(format!("{json}\n").as_bytes()).is_ok() {
                let mut line = String::new();
                if BufReader::new(&stream).read_line(&mut line).is_ok() {
                    if let Some(reply) = Reply::parse(line.trim()) {
                        return Some(reply);
                    }
                }
            }
        }
        let out = Command::new("gdbus")
            .args([
                "call",
                "--session",
                "--dest",
                "org.gnome.Shell",
                "--object-path",
                "/io/github/jeffjose/Jot",
                "--method",
                "io.github.jeffjose.Jot.Quake",
                json,
            ])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let inner = text.trim().strip_prefix("('")?.strip_suffix("',)")?;
        let unescaped = inner.replace("\\'", "'").replace("\\\\", "\\");
        Reply::parse(&unescaped)
    }

    /// Starts the quake window via GNOME Shell's `Meta.WaylandClient` so it
    /// stays out of Alt+Tab, the overview, and the dash/panel.
    pub fn start(exe: &Path) -> bool {
        if !active() {
            return false;
        }
        let config = crate::persist::load_config();
        let shape = Shape::of(&config);
        let exe_str = exe.to_string_lossy();
        let path = std::env::var("PATH").unwrap_or_default();
        let json = format!(
            "{{\"do\":\"start\",\"run\":[{},\"--quake-window\"],\"path\":{},{}}}",
            json_str(&exe_str),
            json_str(&path),
            shape_fields(&shape, config.quake_hide_on_blur)
        );
        if let Some(reply) = ask(&json) {
            return reply.state == "starting" || reply.state == "shown";
        }
        if std::env::var_os("XDG_CURRENT_DESKTOP").is_some_and(|d| d.to_string_lossy().to_ascii_lowercase().contains("gnome")) {
            eprintln!("jot: quake on GNOME Wayland needs jot's GNOME extension (run 'jot setup', then log out and in)");
        }
        false
    }

    /// Tells the shell extension to spawn a fresh `Meta.WaylandClient`
    /// quake window as soon as this one closes (`jot restart`).
    pub fn restart(exe: &str, hidden: bool, config: &crate::persist::Config) -> bool {
        if !active() {
            return false;
        }
        let shape = Shape::of(config);
        let path = std::env::var("PATH").unwrap_or_default();
        let run = if hidden {
            format!("[{},\"--quake-window\",\"--quake-hidden\"]", json_str(exe))
        } else {
            format!("[{},\"--quake-window\"]", json_str(exe))
        };
        let json = format!(
            "{{\"do\":\"restart\",\"run\":{run},\"hidden\":{hidden},\"path\":{},{}}}",
            json_str(&path),
            shape_fields(&shape, config.quake_hide_on_blur)
        );
        ask(&json).is_some_and(|r| r.state == "restarting" || r.state == "starting")
    }

    pub fn toggle(shape: &Shape, blur: bool) -> Option<bool> {
        if !active() {
            return None;
        }
        let json = format!("{{\"do\":\"toggle\",{}}}", shape_fields(shape, blur));
        Some(ask(&json)?.state == "shown")
    }

    pub fn show(shape: &Shape, blur: bool) -> bool {
        if !active() {
            return false;
        }
        let json = format!("{{\"do\":\"show\",{}}}", shape_fields(shape, blur));
        ask(&json).is_some()
    }

    pub fn hide() -> bool {
        if !active() {
            return false;
        }
        ask("{\"do\":\"hide\"}").is_some()
    }

    pub fn place(shape: &Shape, blur: bool) -> bool {
        if !active() {
            return false;
        }
        let json = format!("{{\"do\":\"place\",{}}}", shape_fields(shape, blur));
        ask(&json).is_some()
    }

    pub fn place_pixels(x: i32, y: i32, w: i32, h: i32, shape: &Shape, blur: bool) -> bool {
        if !active() {
            return false;
        }
        let json = format!(
            "{{\"do\":\"place\",\"pixels\":[{x},{y},{w},{h}],\"pointer\":{},\"all_workspaces\":{},\"blur\":{blur}}}",
            shape.pointer, shape.all_workspaces
        );
        ask(&json).is_some()
    }

    pub fn pointer_and_area(shape: &Shape, blur: bool) -> Option<((i32, i32), (i32, i32, i32, i32))> {
        if !active() {
            return None;
        }
        let json = format!("{{\"do\":\"state\",{}}}", shape_fields(shape, blur));
        let reply = ask(&json)?;
        Some((reply.pointer?, reply.area?))
    }

    /// GNOME 45+ ES module extension source.
    pub fn es_module_source() -> String {
        format!(
            "import Gio from 'gi://Gio';\n\
             import GLib from 'gi://GLib';\n\
             import Meta from 'gi://Meta';\n\
             import * as Main from 'resource:///org/gnome/shell/ui/main.js';\n\
             import {{Extension}} from 'resource:///org/gnome/shell/extensions/extension.js';\n\n\
             const VERSION = {VERSION};\n\
             const INTERFACE = `{INTERFACE}`;\n\n\
             export default class JotExtension extends Extension {{\n\
             {BODY}\n\
             }}\n"
        )
    }

    /// GNOME 40-44 legacy `imports.gi` extension source.
    pub fn legacy_source() -> String {
        format!(
            "const {{Gio, GLib, Meta}} = imports.gi;\n\
             const Main = imports.ui.main;\n\n\
             const VERSION = {VERSION};\n\
             const INTERFACE = `{INTERFACE}`;\n\n\
             class JotExtension {{\n\
             {BODY}\n\
             }}\n\n\
             function init() {{\n    return new JotExtension();\n}}\n"
        )
    }

    fn gnome_major() -> Option<u32> {
        let out = Command::new("gnome-shell").arg("--version").output().ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        text.split_whitespace().find_map(|tok| tok.split('.').next()?.parse::<u32>().ok())
    }

    fn enable_in_gsettings() {
        let quiet = || std::process::Stdio::null();
        let _ = Command::new("gnome-extensions").args(["enable", EXTENSION_UUID]).stdout(quiet()).stderr(quiet()).status();
        let Ok(out) = Command::new("gsettings").args(["get", "org.gnome.shell", "enabled-extensions"]).output() else {
            return;
        };
        if !out.status.success() {
            return;
        }
        let cur = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if cur.contains(EXTENSION_UUID) {
            return;
        }
        let next = if cur.starts_with("@as ") || cur == "[]" {
            format!("['{EXTENSION_UUID}']")
        } else if let Some(stripped) = cur.strip_suffix(']') {
            format!("{stripped}, '{EXTENSION_UUID}']")
        } else {
            return;
        };
        let _ = Command::new("gsettings").args(["set", "org.gnome.shell", "enabled-extensions", &next]).stdout(quiet()).stderr(quiet()).status();
    }

    /// Called by `jot setup`: writes and enables `jot@jeffjose.github.io`
    /// when GNOME Shell is installed on the machine.
    pub fn install_extension(data_home: &Path) {
        let Some(major) = gnome_major() else { return };
        let dir = data_home.join("gnome-shell/extensions").join(EXTENSION_UUID);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("jot: could not create {}: {e}", dir.display());
            return;
        }
        let versions = if major >= 45 {
            r#"["45", "46", "47", "48", "49", "50"]"#
        } else {
            r#"["40", "41", "42", "43", "44"]"#
        };
        let metadata = format!(
            "{{\n  \"uuid\": \"{EXTENSION_UUID}\",\n  \"name\": \"jot Quake\",\n  \"description\": \"Places and toggles jot's quake terminal on GNOME Wayland\",\n  \"shell-version\": {versions},\n  \"version\": {VERSION}\n}}\n"
        );
        let code = if major >= 45 { es_module_source() } else { legacy_source() };
        for (path, bytes) in [(dir.join("metadata.json"), metadata.as_bytes()), (dir.join("extension.js"), code.as_bytes())] {
            match std::fs::write(&path, bytes) {
                Ok(()) => println!("wrote {}", path.display()),
                Err(e) => eprintln!("jot: could not write {}: {e}", path.display()),
            }
        }
        enable_in_gsettings();
        if active() && ask("{\"do\":\"state\"}").is_none() {
            println!("note: log out and back in once so GNOME Shell loads {EXTENSION_UUID}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_rect_respects_edges_and_position() {
        let mut c = crate::persist::Config::default();
        c.quake_height = 40;
        c.quake_width = 100;
        c.quake_position = "top".into();
        c.quake_left = -1.0;
        c.quake_right = -1.0;
        let s = Shape::of(&c);
        assert_eq!(s.rect(), [0.0, 0.0, 1.0, 0.4]);
        assert!(!s.narrowed());

        c.quake_width = 60;
        c.quake_align = "center".into();
        c.quake_position = "bottom".into();
        let s = Shape::of(&c);
        let [x, y, w, h] = s.rect();
        assert!((x - 0.2).abs() < 1e-4);
        assert!((y - 0.6).abs() < 1e-4);
        assert!((w - 0.6).abs() < 1e-4);
        assert!((h - 0.4).abs() < 1e-4);
        assert!(s.narrowed());
    }

    #[test]
    fn wayland_reply_parses_state_and_geometry() {
        let raw = r#"{"version":1,"state":"shown","focused":true,"covered":false,"pointer":[960,540],"area":[0,32,1920,1048],"frame":[0,32,1920,419]}"#;
        let r = wayland::Reply::parse(raw).expect("valid reply");
        assert_eq!(r.state, "shown");
        assert!(r.focused);
        assert!(!r.covered);
        assert_eq!(r.pointer, Some((960, 540)));
        assert_eq!(r.area, Some((0, 32, 1920, 1048)));
        assert_eq!(r.frame, Some((0, 32, 1920, 419)));
    }

    #[test]
    fn extension_runs_in_mock_gnome_shell() {
        if std::process::Command::new("node").arg("--version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir();
        let path = dir.join(format!("jot-ext-{}.js", std::process::id()));
        std::fs::write(&path, wayland::legacy_source()).expect("write extension");
        let test_script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/extension_shell.js");
        let out = std::process::Command::new("node").arg(&test_script).arg(&path).output().expect("run node");
        let _ = std::fs::remove_file(&path);
        assert!(
            out.status.success(),
            "extension_shell.js failed:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
