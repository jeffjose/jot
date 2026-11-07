//! The window: workspaces of panes, each pane a program on a terminal of
//! its own, with screen's keys to split, move between and close them.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use softbuffer::{Context, Surface};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{CursorIcon, Window, WindowId};

use crate::canvas::{mix, Canvas};
use crate::font::Fonts;
use crate::layout::{neighbour, Divider, Dir, Node, PaneId, Rect};
use crate::persist::{self, Config, Session, Start, WorkspaceSpec};
use crate::pty::{self, Output, Pty};
use crate::vt::{self, sys, Cell, Terminal};

// Colours from Radix, as the screen setup has them.
const BG: u32 = 0x111111;
const FG: u32 = 0xEEEEEE;
const DIVIDER: u32 = 0x3A3A3A;
const ACTIVE_BG: u32 = 0xEC5D5E; // red 10
const ACTIVE_FG: u32 = 0x191111;
const IDLE_BG: u32 = 0x2A2A2A; // gray 4
const IDLE_FG: u32 = 0x8D8D8D; // gray 9
const STATUS_BG: u32 = 0x111111;
const STATUS_FG: u32 = 0xB4B4B4;
const CURRENT_BG: u32 = 0x30A46C; // green 9
const CURRENT_FG: u32 = 0x0E1512;
const BADGE_BG: u32 = 0x8EC8F6; // blue 7
const BADGE_FG: u32 = 0x113264;
const ACTIVITY: u32 = 0xFFCA16; // amber
const BELL: u32 = 0xFF9592; // red 11
const SCROLLED: u32 = 0xFFC53D;
const SELECTION: u32 = 0x0D3868; // blue 5
const ROAM_BG: u32 = 0x6E56CF; // violet 9

pub struct Pane {
    // The terminal is dropped before the PTY its replies are written to.
    term: Terminal,
    pty: Pty,
    /// The command it was started with, "" for the shell: what is saved.
    cmd: String,
    /// Where its text goes, inside its rectangle.
    content: Rect,
    rect: Rect,
    cols: u16,
    rows: u16,
    /// Every row to be drawn, not only the ones that changed.
    full: bool,
    cursor_was: Option<u16>,
    caption: String,
    /// Wrote something while its workspace was not shown.
    activity: bool,
    /// When it started: what it prints at first, its prompt, is not news.
    started: Instant,
}

pub struct Workspace {
    name: String,
    root: Node,
    focus: PaneId,
    zoom: Option<PaneId>,
}

enum Drag {
    Divider(Divider),
    Select { pane: PaneId, from: (u16, u16) },
}

pub struct App {
    config: Config,
    proxy: EventLoopProxy<()>,
    tx: Sender<Output>,
    rx: Receiver<Output>,
    /// Set while a wake is on its way, so a burst of output wakes the
    /// window once.
    woken: Arc<AtomicBool>,
    window: Option<Rc<Window>>,
    surface: Option<Surface<Rc<Window>, Rc<Window>>>,
    canvas: Canvas,
    fonts: Fonts,
    font_size: f32,
    scale: f32,
    panes: HashMap<PaneId, Pane>,
    workspaces: Vec<Workspace>,
    current: usize,
    /// The roaming terminal: one for all workspaces, shown over them.
    roam: Option<PaneId>,
    roam_shown: bool,
    roam_cwd: Option<PathBuf>,
    next_id: PaneId,
    dividers: Vec<Divider>,
    /// Everything to be drawn again: the layout changed.
    full: bool,
    damage: Vec<Rect>,
    mods: ModifiersState,
    pointer: (i32, i32),
    drag: Option<Drag>,
    last_click: Option<(Instant, (u16, u16))>,
    clipboard: Option<arboard::Clipboard>,
    /// Workspace tabs on the status line, for clicks.
    tabs: Vec<(Rect, usize)>,
    saved_at: Instant,
    unsaved: bool,
    captions_at: Instant,
    clock: String,
    session: Option<Session>,
}

fn hostname() -> String {
    let mut buf = [0u8; 256];
    unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    let s = String::from_utf8_lossy(&buf[..buf.iter().position(|&b| b == 0).unwrap_or(0)]).into_owned();
    s.split('.').next().unwrap_or("").to_string()
}

fn clock() -> String {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        let mut buf = [0u8; 64];
        let n = libc::strftime(buf.as_mut_ptr() as *mut libc::c_char, buf.len(), c"%b %d %H:%M".as_ptr(), &tm);
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }
}

fn folder_name(p: &std::path::Path) -> String {
    if p == pty::home() {
        return "~".into();
    }
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

impl App {
    pub fn new(proxy: EventLoopProxy<()>, config: Config, session: Option<Session>) -> App {
        let (tx, rx) = channel();
        let font_size = config.font_size;
        App {
            fonts: Fonts::new(font_size, font_size * 0.9),
            font_size,
            config,
            proxy,
            tx,
            rx,
            woken: Arc::new(AtomicBool::new(false)),
            window: None,
            surface: None,
            canvas: Canvas::new(),
            scale: 1.0,
            panes: HashMap::new(),
            workspaces: Vec::new(),
            current: 0,
            roam: None,
            roam_shown: false,
            roam_cwd: None,
            next_id: 1,
            dividers: Vec::new(),
            full: true,
            damage: Vec::new(),
            mods: ModifiersState::empty(),
            pointer: (0, 0),
            drag: None,
            last_click: None,
            clipboard: None,
            tabs: Vec::new(),
            saved_at: Instant::now(),
            unsaved: false,
            captions_at: Instant::now() - Duration::from_secs(10),
            clock: clock(),
            session,
        }
    }

    fn px(&self, v: f32) -> i32 {
        (v * self.scale).round() as i32
    }

    fn status_h(&self) -> i32 {
        self.fonts.ui.height + self.px(8.0)
    }

    fn caption_h(&self) -> i32 {
        self.fonts.ui.height + self.px(6.0)
    }

    /// Starts a pane: its program on a PTY, and the terminal it writes to.
    fn spawn(&mut self, start: Start) -> PaneId {
        let id = self.next_id;
        self.next_id += 1;
        let (cols, rows) = (80, 24);
        let pty = match Pty::spawn(&start.cmd, &start.cwd, cols, rows, (0, 0)) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("jterm: could not start a shell: {e}");
                std::process::exit(1);
            }
        };
        let fd = pty.fd;
        let reply: vt::Reply = Box::new(move |bytes: &[u8]| unsafe {
            libc::write(fd, bytes.as_ptr() as *const libc::c_void, bytes.len());
        });
        let term = Terminal::new(cols, rows, self.config.scrollback, FG, BG, reply);
        let (tx, proxy, woken) = (self.tx.clone(), self.proxy.clone(), self.woken.clone());
        pty.read_on_thread(id, move |out| {
            let _ = tx.send(out);
            if !woken.swap(true, Ordering::AcqRel) {
                let _ = proxy.send_event(());
            }
        });
        let caption = folder_name(&start.cwd);
        self.panes.insert(
            id,
            Pane { term, pty, cmd: start.cmd, content: Rect::default(), rect: Rect::default(), cols, rows, full: true, cursor_was: None, caption, activity: false, started: Instant::now() },
        );
        id
    }

    fn add_workspace(&mut self, spec: &WorkspaceSpec) {
        let mut ids = Vec::new();
        let root = spec.build(&mut |start| {
            let id = self.spawn(start);
            ids.push(id);
            id
        });
        let name = if spec.name.is_empty() { folder_name(&spec.folder()) } else { spec.name.clone() };
        self.workspaces.push(Workspace { name, root, focus: ids[0], zoom: None });
    }

    /// The workspaces to begin with: those saved, else those in the
    /// config, else one shell where jterm was started.
    fn start(&mut self) {
        let session = self.session.take();
        let (specs, current, roam) = match session {
            Some(s) => (s.workspaces, s.current, s.roam),
            None if !self.config.workspaces.is_empty() => (std::mem::take(&mut self.config.workspaces), 0, None),
            None => {
                let cwd = std::env::current_dir().unwrap_or_else(|_| pty::home());
                (vec![WorkspaceSpec { name: String::new(), cwd: Some(cwd.display().to_string()), panes: vec![], split: None, layout: None }], 0, None)
            }
        };
        for spec in &specs {
            self.add_workspace(spec);
        }
        self.current = current.min(self.workspaces.len().saturating_sub(1));
        self.roam_cwd = roam.map(|r| persist::expand(&r));
    }

    fn ws(&self) -> &Workspace {
        &self.workspaces[self.current]
    }

    fn ws_mut(&mut self) -> &mut Workspace {
        &mut self.workspaces[self.current]
    }

    /// The pane keys go to.
    fn focused(&self) -> Option<PaneId> {
        if self.roam_shown {
            return self.roam;
        }
        self.workspaces.get(self.current).map(|w| w.focus)
    }

    fn area(&self) -> Rect {
        Rect { x: 0, y: 0, w: self.canvas.width, h: self.canvas.height - self.status_h() }
    }

    /// Lays out every workspace, so panes not shown are the right size
    /// too, and resizes the terminals whose size changed.
    fn relayout(&mut self) {
        let area = self.area();
        let mut shown = Vec::new();
        for (i, ws) in self.workspaces.iter().enumerate() {
            let (mut panes, mut dividers) = (Vec::new(), Vec::new());
            match ws.zoom {
                Some(z) => panes.push((z, area)),
                None => ws.root.layout(area, &mut panes, &mut dividers),
            }
            if i == self.current {
                self.dividers = dividers;
            }
            shown.extend(panes);
        }
        if let Some(r) = self.roam {
            let m = self.px(24.0);
            shown.push((r, Rect { x: area.x + m, y: area.y + m, w: area.w - 2 * m, h: area.h - 2 * m }));
        }
        let (cw, ch, cap, pad) = (self.fonts.cell_w, self.fonts.cell_h, self.caption_h(), self.px(4.0));
        for (id, rect) in shown {
            let Some(p) = self.panes.get_mut(&id) else { continue };
            let content = Rect { x: rect.x + pad, y: rect.y + pad, w: (rect.w - 2 * pad).max(0), h: (rect.h - cap - pad).max(0) };
            let cols = (content.w / cw).clamp(1, u16::MAX as i32) as u16;
            let rows = (content.h / ch).clamp(1, u16::MAX as i32) as u16;
            if (cols, rows) != (p.cols, p.rows) || p.rect != rect {
                p.term.resize(cols, rows, cw as u32, ch as u32);
                p.pty.resize(cols, rows, ((cols as i32 * cw) as u32, (rows as i32 * ch) as u32));
                p.cols = cols;
                p.rows = rows;
            }
            p.rect = rect;
            p.content = content;
            p.full = true;
        }
        self.full = true;
        self.redraw();
    }

    fn redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn changed(&mut self) {
        self.unsaved = true;
        self.relayout();
    }

    fn split(&mut self, dir: Dir) {
        if self.roam_shown {
            return;
        }
        let at = self.ws().focus;
        let cwd = self.panes.get(&at).and_then(|p| p.pty.cwd()).unwrap_or_else(pty::home);
        let id = self.spawn(Start { cmd: String::new(), cwd });
        let ws = self.ws_mut();
        ws.zoom = None;
        ws.root.split(at, id, dir);
        ws.focus = id;
        self.changed();
    }

    /// Closes a pane, its workspace with it when it was the last; jterm
    /// with the last workspace.
    fn close(&mut self, id: PaneId, event_loop: &ActiveEventLoop) {
        self.panes.remove(&id);
        if self.roam == Some(id) {
            self.roam = None;
            self.roam_shown = false;
            self.full = true;
            self.relayout();
            return;
        }
        let Some(wi) = self.workspaces.iter().position(|w| w.root.contains(id)) else { return };
        let ws = &mut self.workspaces[wi];
        if ws.root.remove(id) {
            if ws.zoom == Some(id) {
                ws.zoom = None;
            }
            if ws.focus == id {
                let mut ids = Vec::new();
                ws.root.panes(&mut ids);
                ws.focus = ids[0];
            }
        } else {
            self.workspaces.remove(wi);
            if self.workspaces.is_empty() {
                self.save();
                event_loop.exit();
                return;
            }
            if self.current >= wi && self.current > 0 {
                self.current -= 1;
            }
        }
        self.changed();
    }

    fn focus_dir(&mut self, dx: i32, dy: i32) {
        if self.roam_shown || self.ws().zoom.is_some() {
            return;
        }
        let (mut panes, mut divs) = (Vec::new(), Vec::new());
        self.ws().root.layout(self.area(), &mut panes, &mut divs);
        if let Some(id) = neighbour(&panes, self.ws().focus, dx, dy) {
            self.set_focus(id);
        }
    }

    fn set_focus(&mut self, id: PaneId) {
        let old = self.ws().focus;
        if old == id {
            return;
        }
        self.ws_mut().focus = id;
        for p in [old, id] {
            if let Some(p) = self.panes.get_mut(&p) {
                p.full = true;
            }
        }
        self.unsaved = true;
        self.redraw();
    }

    fn go_to(&mut self, i: usize) {
        if i < self.workspaces.len() && i != self.current {
            self.current = i;
            self.roam_shown = false;
            let mut ids = Vec::new();
            self.ws().root.panes(&mut ids);
            for id in ids {
                if let Some(p) = self.panes.get_mut(&id) {
                    p.activity = false;
                }
            }
            self.changed();
        }
    }

    fn new_workspace(&mut self) {
        let cwd = self.focused().and_then(|f| self.panes.get(&f)).and_then(|p| p.pty.cwd()).unwrap_or_else(pty::home);
        self.add_workspace(&WorkspaceSpec { name: String::new(), cwd: Some(cwd.display().to_string()), panes: vec![], split: None, layout: None });
        self.go_to(self.workspaces.len() - 1);
    }

    fn toggle_roam(&mut self) {
        if self.roam.is_none() {
            let cwd = self.roam_cwd.clone().unwrap_or_else(pty::home);
            self.roam = Some(self.spawn(Start { cmd: String::new(), cwd }));
        }
        self.roam_shown = !self.roam_shown;
        self.unsaved = true;
        self.relayout();
    }

    /// Every pane but the focused one closed: screen's `only`.
    fn only(&mut self) {
        if self.roam_shown {
            return;
        }
        let keep = self.ws().focus;
        let mut ids = Vec::new();
        self.ws().root.panes(&mut ids);
        for id in ids.into_iter().filter(|&i| i != keep) {
            self.panes.remove(&id);
        }
        let ws = self.ws_mut();
        ws.root = Node::Pane(keep);
        ws.zoom = None;
        self.changed();
    }

    fn set_font_size(&mut self, size: f32) {
        self.font_size = size.clamp(6.0, 48.0);
        self.fonts = Fonts::new(self.font_size * self.scale, self.font_size * 0.9 * self.scale);
        self.relayout();
    }

    fn copy(&mut self, primary: bool) {
        let Some(text) = self.focused().and_then(|f| self.panes.get(&f)).and_then(|p| p.term.selection_text()) else { return };
        let cb = self.clipboard.get_or_insert_with(|| arboard::Clipboard::new().expect("clipboard"));
        if primary {
            use arboard::SetExtLinux;
            let _ = cb.set().clipboard(arboard::LinuxClipboardKind::Primary).text(text);
        } else {
            let _ = cb.set_text(text);
        }
    }

    fn paste(&mut self, primary: bool) {
        let cb = self.clipboard.get_or_insert_with(|| arboard::Clipboard::new().expect("clipboard"));
        let text = if primary {
            use arboard::GetExtLinux;
            cb.get().clipboard(arboard::LinuxClipboardKind::Primary).text()
        } else {
            cb.get_text()
        };
        let (Ok(text), Some(id)) = (text, self.focused()) else { return };
        if let Some(p) = self.panes.get_mut(&id) {
            let bytes = p.term.encode_paste(&text);
            p.pty.write(&bytes);
            p.term.scroll_to_bottom();
        }
    }

    /// The keys jterm keeps for itself, screen's among them. True when
    /// the key was one.
    fn binding(&mut self, code: KeyCode, event_loop: &ActiveEventLoop) -> bool {
        let m = self.mods;
        let (alt, ctrl, shift, sup) = (m.alt_key(), m.control_key(), m.shift_key(), m.super_key());
        if alt && !ctrl && !sup {
            match (code, shift) {
                (KeyCode::KeyS, false) => self.split(Dir::Down),
                (KeyCode::KeyV, false) => self.split(Dir::Across),
                (KeyCode::KeyH, false) => self.focus_dir(-1, 0),
                (KeyCode::KeyJ, false) => self.focus_dir(0, 1),
                (KeyCode::KeyK, false) => self.focus_dir(0, -1),
                (KeyCode::KeyL, false) => self.focus_dir(1, 0),
                (KeyCode::KeyX, false) => {
                    if let Some(id) = self.focused() {
                        self.close(id, event_loop);
                    }
                }
                (KeyCode::KeyM, true) => {
                    if !self.roam_shown {
                        let ws = self.ws_mut();
                        ws.zoom = if ws.zoom.is_some() { None } else { Some(ws.focus) };
                        self.changed();
                    }
                }
                (KeyCode::Equal, false) => {
                    self.ws_mut().root.equalize();
                    self.changed();
                }
                (KeyCode::ArrowLeft | KeyCode::ArrowRight | KeyCode::ArrowUp | KeyCode::ArrowDown, _) => {
                    let (dir, by) = match code {
                        KeyCode::ArrowLeft => (Dir::Across, -0.05),
                        KeyCode::ArrowRight => (Dir::Across, 0.05),
                        KeyCode::ArrowUp => (Dir::Down, 0.05),
                        _ => (Dir::Down, -0.05),
                    };
                    let focus = self.ws().focus;
                    if !self.roam_shown && self.ws_mut().root.grow(focus, dir, by) {
                        self.changed();
                    }
                }
                (KeyCode::Digit1 | KeyCode::Digit2 | KeyCode::Digit3 | KeyCode::Digit4 | KeyCode::Digit5 | KeyCode::Digit6 | KeyCode::Digit7 | KeyCode::Digit8 | KeyCode::Digit9, false) => {
                    let n = code as usize - KeyCode::Digit1 as usize;
                    self.go_to(n);
                }
                (KeyCode::Comma, _) => self.go_to((self.current + self.workspaces.len() - 1) % self.workspaces.len()),
                (KeyCode::Period, _) => self.go_to((self.current + 1) % self.workspaces.len()),
                (KeyCode::KeyN, true) => self.new_workspace(),
                (KeyCode::KeyP, true) => self.paste(false),
                (KeyCode::Backquote, false) => self.toggle_roam(),
                _ => return false,
            }
            return true;
        }
        if sup && !alt && !ctrl && code == KeyCode::KeyK {
            if let Some(p) = self.focused().and_then(|f| self.panes.get_mut(&f)) {
                p.term.clear();
                // The shell draws its prompt again at the top.
                p.pty.write(b"\x0c");
                p.full = true;
                self.redraw();
            }
            return true;
        }
        if ctrl && shift && !alt {
            match code {
                KeyCode::KeyC => self.copy(false),
                KeyCode::KeyV => self.paste(false),
                _ => return false,
            }
            return true;
        }
        if ctrl && !shift && !alt && code == KeyCode::KeyQ {
            self.only();
            return true;
        }
        if ctrl && !alt {
            match code {
                KeyCode::Equal => self.set_font_size(self.font_size + 1.0),
                KeyCode::Minus => self.set_font_size(self.font_size - 1.0),
                KeyCode::Digit0 => self.set_font_size(self.config.font_size),
                _ => return false,
            }
            return true;
        }
        if shift && !ctrl && !alt && matches!(code, KeyCode::PageUp | KeyCode::PageDown) {
            if let Some(p) = self.focused().and_then(|f| self.panes.get_mut(&f)) {
                let page = p.rows as isize - 1;
                p.term.scroll(if code == KeyCode::PageUp { -page } else { page });
                self.redraw();
            }
            return true;
        }
        false
    }

    fn key(&mut self, event: winit::event::KeyEvent, event_loop: &ActiveEventLoop) {
        let PhysicalKey::Code(code) = event.physical_key else { return };
        if event.state == ElementState::Pressed && self.binding(code, event_loop) {
            return;
        }
        let Some(id) = self.focused() else { return };
        let Some(p) = self.panes.get_mut(&id) else { return };
        let action = match (event.state, event.repeat) {
            (ElementState::Released, _) => sys::GHOSTTY_KEY_ACTION_RELEASE,
            (_, true) => sys::GHOSTTY_KEY_ACTION_REPEAT,
            _ => sys::GHOSTTY_KEY_ACTION_PRESS,
        };
        let m = self.mods;
        let mut mods = 0u16;
        for (on, bit) in [(m.shift_key(), sys::GHOSTTY_MODS_SHIFT), (m.control_key(), sys::GHOSTTY_MODS_CTRL), (m.alt_key(), sys::GHOSTTY_MODS_ALT), (m.super_key(), sys::GHOSTTY_MODS_SUPER)] {
            if on {
                mods |= bit as u16;
            }
        }
        let unshifted = match event.key_without_modifiers() {
            winit::keyboard::Key::Character(s) => s.chars().next().map_or(0, |c| c as u32),
            _ => 0,
        };
        let text = event.text.as_deref().unwrap_or("");
        let bytes = p.term.encode_key(crate::keys::ghostty(code), mods, action, text, unshifted);
        if !bytes.is_empty() {
            p.pty.write(&bytes);
            if !p.term.at_bottom() {
                p.term.scroll_to_bottom();
                self.redraw();
            }
        }
    }

    /// The pane under a point, and the cell in it.
    fn pane_at(&self, x: i32, y: i32) -> Option<(PaneId, (u16, u16))> {
        let ids: Vec<PaneId> = if self.roam_shown {
            self.roam.into_iter().collect()
        } else if let Some(z) = self.ws().zoom {
            vec![z]
        } else {
            let mut v = Vec::new();
            self.ws().root.panes(&mut v);
            v
        };
        ids.into_iter().find_map(|id| {
            let p = self.panes.get(&id)?;
            p.rect.contains(x, y).then(|| {
                let cx = ((x - p.content.x).max(0) / self.fonts.cell_w).min(p.cols as i32 - 1) as u16;
                let cy = ((y - p.content.y).max(0) / self.fonts.cell_h).min(p.rows as i32 - 1) as u16;
                (id, (cx, cy))
            })
        })
    }

    fn mouse_press(&mut self, button: MouseButton) {
        let (x, y) = self.pointer;
        if button == MouseButton::Left {
            if let Some(&(_, i)) = self.tabs.iter().find(|(r, _)| r.contains(x, y)) {
                self.go_to(i);
                return;
            }
            if !self.roam_shown {
                let grab = self.px(3.0);
                if let Some(d) = self.dividers.iter().find(|d| Rect { x: d.rect.x - grab, y: d.rect.y - grab, w: d.rect.w + 2 * grab, h: d.rect.h + 2 * grab }.contains(x, y)) {
                    self.drag = Some(Drag::Divider(d.clone()));
                    return;
                }
            }
        }
        let Some((id, cell)) = self.pane_at(x, y) else { return };
        if !self.roam_shown {
            self.set_focus(id);
        }
        match button {
            MouseButton::Left => {
                let Some(p) = self.panes.get_mut(&id) else { return };
                let double = self.last_click.is_some_and(|(t, c)| t.elapsed() < Duration::from_millis(400) && c == cell);
                self.last_click = Some((Instant::now(), cell));
                if double {
                    p.term.select_word(cell);
                    self.drag = None;
                    self.copy(true);
                } else {
                    p.term.clear_selection();
                    self.drag = Some(Drag::Select { pane: id, from: cell });
                }
                self.redraw();
            }
            MouseButton::Middle => self.paste(true),
            _ => {}
        }
    }

    fn mouse_move(&mut self, x: i32, y: i32) {
        self.pointer = (x, y);
        match &self.drag {
            Some(Drag::Divider(d)) => {
                let fraction = match d.dir {
                    Dir::Across => (x - d.span.x) as f32 / d.span.w.max(1) as f32,
                    Dir::Down => (y - d.span.y) as f32 / d.span.h.max(1) as f32,
                };
                let (path, gap) = (d.path.clone(), d.gap);
                self.ws_mut().root.drag(&path, gap, fraction);
                self.changed();
            }
            Some(Drag::Select { pane, from }) => {
                let (pane, from) = (*pane, *from);
                if let Some(p) = self.panes.get_mut(&pane) {
                    let cx = ((x - p.content.x) / self.fonts.cell_w).clamp(0, p.cols as i32 - 1) as u16;
                    let cy = ((y - p.content.y) / self.fonts.cell_h).clamp(0, p.rows as i32 - 1) as u16;
                    if (cx, cy) != from {
                        p.term.select(from, (cx, cy));
                        self.redraw();
                    }
                }
            }
            None => {
                let g = self.px(3.0);
                let on_divider = if self.roam_shown {
                    None
                } else {
                    self.dividers.iter().find(|d| Rect { x: d.rect.x - g, y: d.rect.y - g, w: d.rect.w + 2 * g, h: d.rect.h + 2 * g }.contains(x, y))
                };
                if let Some(w) = &self.window {
                    w.set_cursor(match on_divider {
                        Some(d) if d.dir == Dir::Across => CursorIcon::ColResize,
                        Some(_) => CursorIcon::RowResize,
                        None => CursorIcon::Text,
                    });
                }
            }
        }
    }

    fn mouse_release(&mut self) {
        if let Some(Drag::Select { .. }) = self.drag.take() {
            self.copy(true);
        }
    }

    fn scroll(&mut self, lines: isize) {
        let (x, y) = self.pointer;
        let Some((id, _)) = self.pane_at(x, y) else { return };
        let Some(p) = self.panes.get_mut(&id) else { return };
        // On the alternate screen (a pager, an editor) there is nothing
        // to scroll back to: the wheel moves the cursor, as arrows do.
        if p.term.alternate_screen() {
            let key = if lines < 0 { sys::GHOSTTY_KEY_ARROW_UP } else { sys::GHOSTTY_KEY_ARROW_DOWN };
            for _ in 0..lines.unsigned_abs() {
                let b = p.term.encode_key(key, 0, sys::GHOSTTY_KEY_ACTION_PRESS, "", 0);
                p.pty.write(&b);
            }
        } else {
            p.term.scroll(lines);
        }
        self.redraw();
    }

    /// Reads in what the panes wrote.
    fn drain(&mut self, event_loop: &ActiveEventLoop) {
        self.woken.store(false, Ordering::Release);
        let mut exited = Vec::new();
        let shown: Vec<PaneId> = {
            let mut v = Vec::new();
            if let Some(ws) = self.workspaces.get(self.current) {
                ws.root.panes(&mut v);
            }
            v.extend(self.roam);
            v
        };
        let mut any = false;
        while let Ok(out) = self.rx.try_recv() {
            match out {
                Output::Bytes(id, bytes) => {
                    if let Some(p) = self.panes.get_mut(&id) {
                        p.term.write(&bytes);
                        if !shown.contains(&id) && !p.activity && p.started.elapsed() > Duration::from_secs(3) {
                            p.activity = true;
                            self.full = true;
                        }
                        any = true;
                    }
                }
                Output::Exit(id) => exited.push(id),
            }
        }
        for id in exited {
            if self.panes.contains_key(&id) {
                self.close(id, event_loop);
            }
        }
        pty::reap();
        if any {
            self.redraw();
        }
    }

    /// The captions: the title the program set, else the folder and
    /// what is running in it.
    fn refresh_captions(&mut self) {
        if self.captions_at.elapsed() < Duration::from_millis(500) {
            return;
        }
        self.captions_at = Instant::now();
        let mut changed = false;
        for p in self.panes.values_mut() {
            let title = p.term.title();
            let caption = if !title.trim().is_empty() {
                title
            } else {
                let folder = p.pty.cwd().map(|c| folder_name(&c)).unwrap_or_default();
                match p.pty.foreground() {
                    Some(fg) if p.pty.busy() => format!("{folder}: {fg}"),
                    _ => folder,
                }
            };
            if caption != p.caption {
                p.caption = caption;
                changed = true;
            }
        }
        let now = clock();
        if now != self.clock {
            self.clock = now;
            changed = true;
        }
        if changed {
            self.full = true;
        }
    }

    fn draw(&mut self) {
        self.refresh_captions();
        let full = std::mem::take(&mut self.full);
        if full {
            self.canvas.fill(Rect { x: 0, y: 0, w: self.canvas.width, h: self.canvas.height }, DIVIDER);
            self.damage.push(Rect { x: 0, y: 0, w: self.canvas.width, h: self.canvas.height });
        }
        let ids: Vec<PaneId> = if let Some(z) = self.ws().zoom {
            vec![z]
        } else {
            let mut v = Vec::new();
            self.ws().root.panes(&mut v);
            v
        };
        let focus = self.focused();
        // Under the roaming terminal, panes are drawn only when all of it
        // is, so that what they write does not land on top of it.
        if full || !self.roam_shown {
            for (n, id) in ids.iter().enumerate() {
                self.draw_pane(*id, n + 1, full, Some(*id) == focus && !self.roam_shown);
            }
        }
        if self.roam_shown {
            if let Some(r) = self.roam {
                if full {
                    let rect = self.panes[&r].rect;
                    let b = self.px(2.0);
                    self.canvas.fill(Rect { x: rect.x - b, y: rect.y - b, w: rect.w + 2 * b, h: rect.h + 2 * b }, ROAM_BG);
                }
                self.draw_pane(r, 0, full, true);
            }
        }
        if full {
            self.draw_status();
        }
        self.present();
    }

    fn draw_pane(&mut self, id: PaneId, n: usize, full: bool, focused: bool) {
        let (cw, ch) = (self.fonts.cell_w, self.fonts.cell_h);
        let cap_h = self.caption_h();
        let Some(p) = self.panes.get_mut(&id) else { return };
        let full = full || p.full;
        p.full = false;
        let dirty = p.term.update();
        if dirty == 0 && !full {
            return;
        }
        let (dfg, dbg) = p.term.colors();
        let cursor = p.term.cursor();
        let rect = p.rect;
        let content = p.content;
        if full {
            self.canvas.fill(Rect { x: rect.x, y: rect.y, w: rect.w, h: rect.h - cap_h }, dbg);
        }
        let mut extra = Vec::new();
        extra.extend(p.cursor_was);
        if cursor.visible {
            extra.push(cursor.y);
        }
        p.cursor_was = cursor.visible.then_some(cursor.y);
        let mut row = Vec::new();
        let canvas = &mut self.canvas;
        let fonts = &mut self.fonts;
        let damage = &mut self.damage;
        p.term.rows(full || dirty == 2, &extra, &mut row, |y, cells: &[Cell]| {
            let top = content.y + y as i32 * ch;
            if top + ch > content.y + content.h + ch {
                return;
            }
            // The row across the whole pane, so its edges match.
            let line = Rect { x: rect.x, y: top, w: rect.w, h: ch };
            canvas.fill(line, dbg);
            let clip = Rect { x: rect.x, y: top, w: rect.w, h: ch };
            let colors = |c: &Cell| {
                let (mut fg, mut bg) = (c.fg.unwrap_or(dfg), c.bg.unwrap_or(dbg));
                if c.flags & vt::INVERSE != 0 {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if c.selected {
                    bg = SELECTION;
                }
                if c.flags & vt::FAINT != 0 {
                    fg = mix(bg, fg, 150);
                }
                (fg, bg)
            };
            for (x, c) in cells.iter().enumerate() {
                let (_, bg) = colors(c);
                if bg != dbg {
                    canvas.fill(Rect { x: content.x + x as i32 * cw, y: top, w: cw, h: ch }, bg);
                }
            }
            for (x, c) in cells.iter().enumerate() {
                let left = content.x + x as i32 * cw;
                let (mut fg, bg) = colors(c);
                let on_cursor = cursor.visible && focused && cursor.y == y && cursor.x as usize == x && cursor.style == sys::GHOSTTY_CURSOR_STYLE_BLOCK;
                if on_cursor {
                    canvas.fill(Rect { x: left, y: top, w: cw, h: ch }, cursor.color.unwrap_or(FG));
                    fg = bg;
                }
                if c.flags & vt::INVISIBLE == 0 {
                    if let Some(chr) = c.text.chars().next() {
                        if chr != ' ' {
                            let g = fonts.glyph(chr, c.flags & vt::BOLD != 0);
                            canvas.glyph(g, left, top, fg, clip);
                        }
                    }
                }
                if c.underline() {
                    canvas.fill(Rect { x: left, y: top + ch - (ch / 10).max(2), w: cw, h: (ch / 16).max(1) }, fg);
                }
                if c.flags & vt::STRIKE != 0 {
                    canvas.fill(Rect { x: left, y: top + ch / 2, w: cw, h: (ch / 16).max(1) }, fg);
                }
            }
            if cursor.visible && cursor.y == y {
                let left = content.x + cursor.x as i32 * cw;
                let color = cursor.color.unwrap_or(FG);
                let t = (cw / 8).max(1);
                if !focused || cursor.style == sys::GHOSTTY_CURSOR_STYLE_BLOCK_HOLLOW {
                    canvas.fill(Rect { x: left, y: top, w: cw, h: t }, color);
                    canvas.fill(Rect { x: left, y: top + ch - t, w: cw, h: t }, color);
                    canvas.fill(Rect { x: left, y: top, w: t, h: ch }, color);
                    canvas.fill(Rect { x: left + cw - t, y: top, w: t, h: ch }, color);
                } else if cursor.style == sys::GHOSTTY_CURSOR_STYLE_BAR {
                    canvas.fill(Rect { x: left, y: top, w: (cw / 6).max(2), h: ch }, color);
                } else if cursor.style == sys::GHOSTTY_CURSOR_STYLE_UNDERLINE {
                    canvas.fill(Rect { x: left, y: top + ch - (ch / 8).max(2), w: cw, h: (ch / 8).max(2) }, color);
                }
            }
            damage.push(line);
        });
        // The caption, along the bottom of the pane, as screen's is.
        let caption = p.caption.clone();
        let scrolled = !p.term.at_bottom();
        let activity = p.activity;
        if full {
            self.draw_caption(rect, n, &caption, focused, scrolled, activity);
        }
    }

    fn draw_caption(&mut self, rect: Rect, n: usize, caption: &str, focused: bool, scrolled: bool, activity: bool) {
        let h = self.caption_h();
        let bar = Rect { x: rect.x, y: rect.y + rect.h - h, w: rect.w, h };
        let (bg, fg) = if focused { (ACTIVE_BG, ACTIVE_FG) } else { (IDLE_BG, if activity { ACTIVITY } else { IDLE_FG }) };
        self.canvas.fill(bar, bg);
        let ty = bar.y + (h - self.fonts.ui.height) / 2;
        let pad = self.px(6.0) as f32;
        let label = if n == 0 { format!("~  {caption}") } else { format!("{n}  {caption}") };
        self.canvas.text(&mut self.fonts.ui, &label, bar.x as f32 + pad, ty, fg, bar);
        if scrolled {
            let tag = " SCROLLED ";
            let w = self.fonts.ui.width(tag);
            let r = Rect { x: bar.x + bar.w - w - pad as i32, y: bar.y, w, h };
            self.canvas.fill(r, SCROLLED);
            self.canvas.text(&mut self.fonts.ui, tag, r.x as f32, ty, ACTIVE_FG, r);
        }
        self.damage.push(bar);
    }

    fn draw_status(&mut self) {
        let h = self.status_h();
        let bar = Rect { x: 0, y: self.canvas.height - h, w: self.canvas.width, h };
        self.canvas.fill(bar, STATUS_BG);
        let ty = bar.y + (h - self.fonts.ui.height) / 2;
        let pad = self.px(8.0);
        let mut x = 0;
        let badge = |canvas: &mut Canvas, fonts: &mut Fonts, x: i32, text: &str, bg: u32, fg: u32| -> i32 {
            let w = fonts.ui.width(text) + 2 * pad;
            let r = Rect { x, y: bar.y, w, h };
            canvas.fill(r, bg);
            canvas.text(&mut fonts.ui, text, (x + pad) as f32, ty, fg, r);
            x + w
        };
        x = badge(&mut self.canvas, &mut self.fonts, x, &hostname(), BADGE_BG, BADGE_FG);
        self.tabs.clear();
        for i in 0..self.workspaces.len() {
            let mut ids = Vec::new();
            self.workspaces[i].root.panes(&mut ids);
            let busy = i != self.current && ids.iter().any(|id| self.panes.get(id).is_some_and(|p| p.activity));
            let label = format!("{} {}{}", i + 1, self.workspaces[i].name, if busy { "@" } else { "" });
            let (bg, fg) = if i == self.current { (CURRENT_BG, CURRENT_FG) } else { (STATUS_BG, if busy { ACTIVITY } else { STATUS_FG }) };
            let start = x;
            x = badge(&mut self.canvas, &mut self.fonts, x, &label, bg, fg);
            self.tabs.push((Rect { x: start, y: bar.y, w: x - start, h }, i));
        }
        if self.roam.is_some() {
            let fg = if self.roam_shown { 0xFFFFFF } else { STATUS_FG };
            let bg = if self.roam_shown { ROAM_BG } else { STATUS_BG };
            badge(&mut self.canvas, &mut self.fonts, x, "~ roam", bg, fg);
        }
        let _ = BELL;
        let clock = self.clock.clone();
        let w = self.fonts.ui.width(&clock) + 2 * pad;
        let right = self.canvas.width - w;
        badge(&mut self.canvas, &mut self.fonts, right, &clock, BADGE_BG, BADGE_FG);
        self.damage.push(bar);
    }

    fn present(&mut self) {
        let (Some(surface), Some(window)) = (self.surface.as_mut(), self.window.as_ref()) else { return };
        let damage = std::mem::take(&mut self.damage);
        if damage.is_empty() {
            return;
        }
        let Ok(mut buffer) = surface.buffer_mut() else { return };
        let width = self.canvas.width as usize;
        if buffer.len() != self.canvas.pixels.len() {
            return;
        }
        let whole = buffer.age() != 1 || damage.iter().any(|d| d.w == self.canvas.width && d.h == self.canvas.height);
        let rects: Vec<softbuffer::Rect> = if whole {
            buffer.copy_from_slice(&self.canvas.pixels);
            vec![]
        } else {
            damage
                .iter()
                .filter_map(|d| {
                    let x0 = d.x.clamp(0, self.canvas.width) as usize;
                    let x1 = (d.x + d.w).clamp(0, self.canvas.width) as usize;
                    let y0 = d.y.clamp(0, self.canvas.height) as usize;
                    let y1 = (d.y + d.h).clamp(0, self.canvas.height) as usize;
                    if x0 >= x1 || y0 >= y1 {
                        return None;
                    }
                    for y in y0..y1 {
                        buffer[y * width + x0..y * width + x1].copy_from_slice(&self.canvas.pixels[y * width + x0..y * width + x1]);
                    }
                    Some(softbuffer::Rect { x: x0 as u32, y: y0 as u32, width: NonZeroU32::new((x1 - x0) as u32)?, height: NonZeroU32::new((y1 - y0) as u32)? })
                })
                .collect()
        };
        window.pre_present_notify();
        let _ = if whole || rects.is_empty() { buffer.present() } else { buffer.present_with_damage(&rects) };
    }

    /// The workspaces as they are now, written out.
    fn save(&mut self) {
        let panes = &self.panes;
        let describe = |id: PaneId| match panes.get(&id) {
            Some(p) => (p.cmd.clone(), p.pty.cwd()),
            None => (String::new(), None),
        };
        let workspaces = self
            .workspaces
            .iter()
            .map(|ws| WorkspaceSpec { name: ws.name.clone(), cwd: None, panes: vec![], split: None, layout: Some(persist::spec(&ws.root, &describe)) })
            .collect();
        let roam = self.roam.and_then(|r| panes.get(&r)).and_then(|p| p.pty.cwd()).or(self.roam_cwd.clone()).map(|p| persist::shorten(&p));
        persist::save_session(&Session { current: self.current, roam, workspaces });
        self.saved_at = Instant::now();
        self.unsaved = false;
    }
}

impl ApplicationHandler<()> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title("jterm").with_inner_size(LogicalSize::new(1400.0, 900.0));
        let window = Rc::new(event_loop.create_window(attrs).expect("failed to create window"));
        let context = Context::new(window.clone()).expect("failed to connect to display");
        self.surface = Some(Surface::new(&context, window.clone()).expect("failed to create surface"));
        self.scale = window.scale_factor() as f32;
        self.fonts = Fonts::new(self.font_size * self.scale, self.font_size * 0.9 * self.scale);
        let size = window.inner_size();
        self.canvas.resize(size.width as i32, size.height as i32);
        if let Some(s) = self.surface.as_mut() {
            let _ = s.resize(NonZeroU32::new(size.width.max(1)).unwrap(), NonZeroU32::new(size.height.max(1)).unwrap());
        }
        self.window = Some(window);
        self.start();
        self.relayout();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _: ()) {
        self.drain(event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.save();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(s) = self.surface.as_mut() {
                    let _ = s.resize(NonZeroU32::new(size.width.max(1)).unwrap(), NonZeroU32::new(size.height.max(1)).unwrap());
                }
                self.canvas.resize(size.width as i32, size.height as i32);
                self.relayout();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale = scale_factor as f32;
                self.set_font_size(self.font_size);
            }
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => self.key(event, event_loop),
            WindowEvent::CursorMoved { position, .. } => self.mouse_move(position.x as i32, position.y as i32),
            WindowEvent::MouseInput { state: ElementState::Pressed, button, .. } => self.mouse_press(button),
            WindowEvent::MouseInput { state: ElementState::Released, .. } => self.mouse_release(),
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -(y * 3.0) as isize,
                    MouseScrollDelta::PixelDelta(p) => -(p.y / self.fonts.cell_h as f64) as isize,
                };
                if lines != 0 {
                    self.scroll(lines);
                }
            }
            WindowEvent::Focused(_) => {
                if let Some(id) = self.focused() {
                    if let Some(p) = self.panes.get_mut(&id) {
                        p.full = true;
                    }
                }
                self.redraw();
            }
            WindowEvent::RedrawRequested => self.draw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.unsaved && self.saved_at.elapsed() > Duration::from_secs(2) {
            self.save();
        }
        // Once a second for the captions and the clock, and to save the
        // folders the panes have moved to.
        if self.saved_at.elapsed() > Duration::from_secs(30) {
            self.save();
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_secs(1)));
        if self.captions_at.elapsed() >= Duration::from_secs(1) {
            self.redraw();
        }
    }
}
