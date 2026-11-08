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
        }
    }
}

impl App {
    /// `jot --quake`: shown, or hidden if it shows.
    /// Hidden only when it is the window you are in: shown but under
    /// another window that is also kept on top, it is brought forward instead, as Tilix
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
    /// then every terminal of one machine, those in its panes numbered, the
    /// one you are in lit, any clicked shown in the pane you are in (its
    /// layout, this desk's, never changes: only what its panes show); at
    /// the right that machine, a button to list another's, and hide. Its
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
        let k = self.quake_k.min(self.machines.len().saturating_sub(1));
        let machine = self.machine_name(k);
        let ih = px(10.0);
        let bw = px(8.0) + self.fonts.ui.width(&machine) + px(6.0) + ih + px(7.0);
        let button = Rect { x: hide.x - px(6.0) - bw, y: by, w: bw, h: bh };
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
            let w = (px(8.0) + nw + px(6.0) + self.fonts.ui.width(&label) + px(9.0)).min(px(180.0));
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
            self.canvas.text_fit(&mut self.fonts.ui, &label, cx, ty, r.x + r.w - px(9.0) - cx, color, r);
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
            // As its row in the sidebar: shown in the pane you are in, or
            // gone to when a pane here shows it.
            Some(QuakeHit::Term(hit)) => self.side_row_click(hit),
            Some(QuakeHit::Machine) => self.quake_machine_menu(),
            Some(QuakeHit::Grip) => self.drag = Some(Drag::QuakeGrip),
            Some(QuakeHit::Hide) => self.quake_hide(),
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
