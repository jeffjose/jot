//! The quake window, as Tilix's Quake mode has it: across the top of
//! the monitor the pointer is on, 40% of its height and all its width,
//! above every other window and on every workspace, out of the taskbar.
//! `jot --quake` (a key bound to it) shows it, and hides it again; losing
//! focus does not. It is another window on the session: its panes are its
//! own, and run on while it is hidden.

use super::*;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::window::WindowLevel;

/// Tilix's defaults: the share of the monitor's height and width it takes.
const HEIGHT_PERCENT: u32 = 40;
const WIDTH_PERCENT: u32 = 100;

/// The quake window: no frame, over the others, kept hidden until it
/// is placed.
pub(super) fn attributes() -> winit::window::WindowAttributes {
    use winit::platform::x11::WindowAttributesExtX11;
    let attrs = window_attributes("jot", LogicalSize::new(1200.0, 400.0)).with_decorations(false).with_visible(false).with_window_level(WindowLevel::AlwaysOnTop);
    WindowAttributesExtX11::with_name(attrs, "jot-quake", "jot-quake")
}

impl App {
    /// `jot --quake`: shown, or hidden if it shows.
    pub(super) fn quake_toggle(&mut self) {
        if self.quake_shown {
            self.quake_hide();
        } else {
            self.quake_show();
        }
    }

    /// Shown across the top of the monitor the pointer is on, and given
    /// the keys.
    pub(super) fn quake_show(&mut self) {
        let Some(window) = self.window.clone() else { return };
        let x = x11::X::open();
        let xid = x11_id(&window);
        let place = quake_place(&window, x.as_ref());
        if let Some((pos, size)) = place {
            let _ = window.request_inner_size(size);
            window.set_outer_position(pos);
        }
        if let (Some(x), Some(id)) = (&x, xid) {
            x.before_map(id);
        }
        window.set_visible(true);
        if let (Some(x), Some(id)) = (&x, xid) {
            x.wait_mapped(id);
            // Where the window manager put it, put right.
            if let Some((pos, _)) = place {
                window.set_outer_position(pos);
            }
            x.after_map(id);
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
}

/// Where the quake window goes, and how big: the top of the work area of the
/// monitor the pointer is on (the panels left clear), centred.
fn quake_place(window: &Window, x: Option<&x11::X>) -> Option<(PhysicalPosition<i32>, PhysicalSize<u32>)> {
    let monitors: Vec<_> = window.available_monitors().collect();
    let pointer = x.and_then(|x| x.pointer());
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
    let w = aw * WIDTH_PERCENT as i32 / 100;
    let h = ah * HEIGHT_PERCENT as i32 / 100;
    Some((PhysicalPosition::new(ax + (aw - w) / 2, ay), PhysicalSize::new(w.max(1) as u32, h.max(1) as u32)))
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

        fn states(&self) -> [c_ulong; 4] {
            ["_NET_WM_STATE_ABOVE", "_NET_WM_STATE_STICKY", "_NET_WM_STATE_SKIP_TASKBAR", "_NET_WM_STATE_SKIP_PAGER"].map(|s| self.atom(s))
        }

        /// Said before it is mapped, which is when the window manager reads
        /// them: its states, and all workspaces.
        pub fn before_map(&self, win: u64) {
            let states = self.states();
            let all: [c_ulong; 1] = [0xFFFF_FFFF];
            unsafe {
                (self.lib.XChangeProperty)(self.dpy, win as c_ulong, self.atom("_NET_WM_STATE"), xlib::XA_ATOM, 32, xlib::PropModeReplace, states.as_ptr() as *const c_uchar, states.len() as c_int);
                (self.lib.XChangeProperty)(self.dpy, win as c_ulong, self.atom("_NET_WM_DESKTOP"), xlib::XA_CARDINAL, 32, xlib::PropModeReplace, all.as_ptr() as *const c_uchar, 1);
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
        pub fn after_map(&self, win: u64) {
            let [above, sticky, taskbar, pager] = self.states();
            const ADD: c_long = 1;
            const PAGER: c_long = 2;
            self.message(win, "_NET_WM_STATE", [ADD, above as c_long, sticky as c_long, PAGER, 0]);
            self.message(win, "_NET_WM_STATE", [ADD, taskbar as c_long, pager as c_long, PAGER, 0]);
            self.message(win, "_NET_WM_DESKTOP", [0xFFFF_FFFF, PAGER, 0, 0, 0]);
            self.message(win, "_NET_ACTIVE_WINDOW", [PAGER, 0, 0, 0, 0]);
            unsafe { (self.lib.XFlush)(self.dpy) };
        }
    }
}
