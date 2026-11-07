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
use crate::theme::Theme;
use crate::vt::{self, sys, Cell, Terminal};

mod overlay;
mod copymode;
mod dnd;
mod find;
mod links;
mod mouse;
mod settings;

// Colours from Radix, as the screen setup has them.
const STATUS_BG: u32 = 0x111111;
const STATUS_FG: u32 = 0xB4B4B4;
const CURRENT_BG: u32 = 0x30A46C; // green 9
const CURRENT_FG: u32 = 0x0E1512;
const BADGE_BG: u32 = 0x8EC8F6; // blue 7
const BADGE_FG: u32 = 0x113264;
const ACTIVITY: u32 = 0xFFCA16; // amber
const BELL: u32 = 0xFF9592; // red 11
const SCROLLED: u32 = 0xFFC53D;
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
    /// The theme its terminal has: its workspace's, or the settings'.
    theme: &'static Theme,
    caption: String,
    /// Wrote something while its workspace was not shown.
    activity: bool,
    /// When it started: what it prints at first, its prompt, is not news.
    started: Instant,
    /// The characters of each row as last drawn, to find links in.
    lines: Vec<Vec<char>>,
}

/// A pane closed, to bring back.
struct Closed {
    workspace: String,
    near: Option<(PaneId, Dir, bool)>,
    cmd: String,
    cwd: PathBuf,
}

pub struct Workspace {
    name: String,
    root: Node,
    focus: PaneId,
    zoom: Option<PaneId>,
    /// Its own accent and theme, by name, over the ones in the settings.
    accent: Option<String>,
    theme: Option<String>,
    /// What is typed goes to every pane in it.
    broadcast: bool,
}

enum Drag {
    Divider(Divider),
    Select { pane: PaneId, from: (u16, u16) },
    Pane(dnd::Carry),
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
    theme: &'static Theme,
    /// The colour of what is picked out: the pane you are in, menus.
    accent: u32,
    /// The accent and theme now: the current workspace's own, else the
    /// settings'. What is drawn takes these.
    cur_accent: u32,
    cur_theme: &'static Theme,
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
    palette: Option<crate::palette::Palette>,
    menu: Option<overlay::Menu>,
    settings: Option<settings::Settings>,
    find: Option<find::Find>,
    /// The link lit under the pointer, with Ctrl held.
    link: Option<links::Link>,
    /// A picture of the pane being dragged, carried under the pointer.
    ghost: Option<(Vec<u32>, i32, i32)>,
    /// A button held down in a pane whose program has the mouse.
    mouse_held: Option<(PaneId, sys::GhosttyMouseButton)>,
    copy_mode: Option<copymode::CopyMode>,
    /// Pictures' pixels, by pane and picture, with the generation read.
    pictures: HashMap<(PaneId, u32), (u64, std::rc::Rc<vt::Picture>)>,
    /// What was run from the palette lately, the latest first.
    recent: Vec<String>,
    /// Panes closed, the last last, for Ctrl Shift T.
    closed: Vec<Closed>,
    /// What jterm last put on the clipboard and the selection.
    copied: (Option<String>, Option<String>),
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
        let theme = Theme::by_name(&config.theme);
        let accent = crate::theme::accent(&config.accent);
        App {
            fonts: Fonts::new(&crate::font::Choice { family: config.font_family.clone(), size: font_size, ui_family: config.ui_font_family.clone(), ui_size: config.ui_font_size, line_height: config.line_height, ligatures: config.ligatures }, 1.0),
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
            theme,
            accent,
            cur_accent: accent,
            cur_theme: theme,
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
            palette: None,
            menu: None,
            settings: None,
            find: None,
            link: None,
            ghost: None,
            mouse_held: None,
            copy_mode: None,
            pictures: HashMap::new(),
            recent: Vec::new(),
            closed: Vec::new(),
            copied: (None, None),
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

    /// The status line's height: none unless the config asks for it, so
    /// the panes run to the bottom edge and screen can have its own.
    fn status_h(&self) -> i32 {
        if self.config.status_line {
            self.fonts.ui.height + self.px(8.0)
        } else {
            0
        }
    }

    fn caption_h(&self) -> i32 {
        if !self.config.captions {
            return 0;
        }
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
        let term = Terminal::new(cols, rows, self.config.scrollback, self.theme, reply);
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
            Pane { term, pty, cmd: start.cmd, content: Rect::default(), rect: Rect::default(), cols, rows, full: true, cursor_was: None, theme: self.theme, caption, activity: false, started: Instant::now(), lines: Vec::new() },
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
        self.workspaces.push(Workspace { name, root, focus: ids[0], zoom: None, accent: spec.accent.clone(), theme: spec.theme.clone(), broadcast: false });
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
                (vec![WorkspaceSpec { name: String::new(), cwd: Some(cwd.display().to_string()), panes: vec![], split: None, layout: None, ..Default::default() }], 0, None)
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
        // Room around the text, as VS Code's terminal leaves.
        let (cw, ch, cap, pad, top) = (self.fonts.cell_w, self.fonts.cell_h, self.caption_h(), self.px(12.0), self.px(8.0));
        for (id, rect) in shown {
            let Some(p) = self.panes.get_mut(&id) else { continue };
            let content = Rect { x: rect.x + pad, y: rect.y + top, w: (rect.w - 2 * pad).max(0), h: (rect.h - cap - top).max(0) };
            let cols = (content.w / cw).clamp(1, u16::MAX as i32) as u16;
            let rows = (content.h / ch).clamp(1, u16::MAX as i32) as u16;
            if (cols, rows) != (p.cols, p.rows) || p.rect != rect {
                p.term.resize(cols, rows, cw as u32, ch as u32);
                p.pty.resize(cols, rows, ((cols as i32 * cw) as u32, (rows as i32 * ch) as u32));
                p.cols = cols;
                p.lines.clear();
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
        let at = self.focused().unwrap_or(0);
        let cwd = self.panes.get(&at).and_then(|p| p.pty.cwd()).unwrap_or_else(pty::home);
        self.split_with(dir, Start { cmd: String::new(), cwd });
    }

    /// Splits the pane you are in, the new one running `start`.
    pub(super) fn split_with(&mut self, dir: Dir, start: Start) {
        if self.roam_shown {
            return;
        }
        let at = self.ws().focus;
        let id = self.spawn(start);
        let ws = self.ws_mut();
        ws.zoom = None;
        ws.root.split(at, id, dir);
        ws.focus = id;
        self.changed();
    }

    /// A new pane to the right of the one you are in, as a new tab opens
    /// beside the last, in its folder.
    fn new_pane(&mut self) {
        self.split(Dir::Across);
    }

    /// Closes the pane you are in, as Ctrl W closes a tab, remembering
    /// where it was and what ran in it for Ctrl Shift T.
    fn close_pane(&mut self, event_loop: &ActiveEventLoop) {
        let Some(id) = self.focused() else { return };
        if self.roam != Some(id) {
            if let (Some(ws), Some(p)) = (self.workspaces.iter().find(|w| w.root.contains(id)), self.panes.get(&id)) {
                self.closed.push(Closed { workspace: ws.name.clone(), near: ws.root.place_of(id), cmd: p.cmd.clone(), cwd: p.pty.cwd().unwrap_or_else(pty::home) });
                if self.closed.len() > 20 {
                    self.closed.remove(0);
                }
            }
        }
        self.close(id, event_loop);
    }

    /// The pane closed last, back where it was, its command started
    /// afresh in its folder, as Ctrl Shift T brings back a tab.
    fn reopen(&mut self) {
        let Some(c) = self.closed.pop() else { return };
        let id = self.spawn(Start { cmd: c.cmd, cwd: c.cwd });
        let wi = self.workspaces.iter().position(|w| w.name == c.workspace);
        match wi {
            Some(wi) => {
                let ws = &mut self.workspaces[wi];
                let placed = c.near.is_some_and(|(near, dir, before)| ws.root.split_beside(near, id, dir, before));
                if !placed {
                    let at = ws.focus;
                    ws.root.split(at, id, Dir::Across);
                }
                ws.zoom = None;
                ws.focus = id;
                self.go_to(wi);
            }
            // Its workspace was closed with it: it comes back as one.
            None => {
                self.workspaces.push(Workspace { name: c.workspace, root: Node::Pane(id), focus: id, zoom: None, accent: None, theme: None, broadcast: false });
                self.go_to(self.workspaces.len() - 1);
            }
        }
        self.roam_shown = false;
        self.changed();
    }

    /// Closes a pane, its workspace with it when it was the last; jterm
    /// with the last workspace.
    fn close(&mut self, id: PaneId, event_loop: &ActiveEventLoop) {
        // A search borrows its pane's terminal, so it goes first.
        if self.find.as_ref().is_some_and(|f| f.pane == id) {
            self.find = None;
        }
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
        let beside = ws.root.place_of(id).map(|(near, _, _)| near);
        if ws.root.remove(id) {
            if ws.zoom == Some(id) {
                ws.zoom = None;
            }
            // The pane beside the one closed is the one you are in now.
            if ws.focus == id {
                let mut ids = Vec::new();
                ws.root.panes(&mut ids);
                ws.focus = beside.filter(|b| ids.contains(b)).unwrap_or(ids[0]);
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
        self.add_workspace(&WorkspaceSpec { name: String::new(), cwd: Some(cwd.display().to_string()), panes: vec![], split: None, layout: None, ..Default::default() });
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
        self.find = None;
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

    fn font_choice(&self) -> crate::font::Choice {
        crate::font::Choice { family: self.config.font_family.clone(), size: self.font_size, ui_family: self.config.ui_font_family.clone(), ui_size: self.config.ui_font_size, line_height: self.config.line_height, ligatures: self.config.ligatures }
    }

    /// Every pane drawn again in full, after a setting of how they look
    /// changed.
    fn refresh_panes(&mut self) {
        for p in self.panes.values_mut() {
            p.full = true;
        }
        self.full = true;
        self.redraw();
    }

    /// The fonts made again, after a setting of theirs changed.
    fn reload_fonts(&mut self) {
        self.fonts = Fonts::new(&self.font_choice(), self.scale);
        self.relayout();
    }

    fn set_font_size(&mut self, size: f32) {
        self.font_size = size.clamp(6.0, 48.0);
        self.fonts = Fonts::new(&self.font_choice(), self.scale);
        self.relayout();
    }

    /// Puts text on the clipboard, or on the selection (what a middle
    /// click pastes), and keeps it: reading back what jterm itself put
    /// there can come back empty on X11.
    pub(super) fn clip_set(&mut self, text: String, primary: bool) {
        let cb = self.clipboard.get_or_insert_with(|| arboard::Clipboard::new().expect("clipboard"));
        if primary {
            use arboard::SetExtLinux;
            let _ = cb.set().clipboard(arboard::LinuxClipboardKind::Primary).text(text.clone());
            self.copied.1 = Some(text);
        } else {
            let _ = cb.set_text(text.clone());
            self.copied.0 = Some(text);
        }
    }

    /// What is on the clipboard or the selection: another program's, or
    /// else what jterm put there last.
    pub(super) fn clip_get(&mut self, primary: bool) -> Option<String> {
        let cb = self.clipboard.get_or_insert_with(|| arboard::Clipboard::new().expect("clipboard"));
        let got = if primary {
            use arboard::GetExtLinux;
            cb.get().clipboard(arboard::LinuxClipboardKind::Primary).text()
        } else {
            cb.get_text()
        };
        got.ok().filter(|t| !t.is_empty()).or_else(|| if primary { self.copied.1.clone() } else { self.copied.0.clone() })
    }

    fn copy(&mut self, primary: bool) {
        let Some(text) = self.focused().and_then(|f| self.panes.get(&f)).and_then(|p| p.term.selection_text()) else { return };
        self.clip_set(text, primary);
    }

    /// The panes what is typed goes to: the one you are in, or, with
    /// broadcast on, every pane of its workspace.
    pub(super) fn input_targets(&self, id: PaneId) -> Vec<PaneId> {
        match self.workspaces.get(self.current) {
            Some(ws) if ws.broadcast && !self.roam_shown && ws.root.contains(id) => {
                let mut ids = Vec::new();
                ws.root.panes(&mut ids);
                ids
            }
            _ => vec![id],
        }
    }

    /// Typing into every pane of the workspace at once, or back to one.
    pub(super) fn toggle_broadcast(&mut self) {
        if let Some(ws) = self.workspaces.get_mut(self.current) {
            ws.broadcast = !ws.broadcast;
        }
        self.refresh_panes();
    }

    fn paste(&mut self, primary: bool) {
        let (Some(text), Some(id)) = (self.clip_get(primary), self.focused()) else { return };
        for id in self.input_targets(id) {
            let Some(p) = self.panes.get_mut(&id) else { continue };
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
                    self.close_pane(event_loop);
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
                (KeyCode::KeyB, true) => self.toggle_broadcast(),
                (KeyCode::KeyP, true) => self.paste(false),
                (KeyCode::KeyP, false) => self.open_copy_mode(),
                (KeyCode::Backquote, false) => self.toggle_roam(),
                _ => return false,
            }
            return true;
        }
        if sup && !alt && !ctrl && code == KeyCode::KeyK {
            self.open_palette();
            return true;
        }
        if ctrl && shift && !alt {
            match code {
                KeyCode::KeyC => self.copy(false),
                KeyCode::KeyV => self.paste(false),
                KeyCode::KeyT => self.reopen(),
                _ => return false,
            }
            return true;
        }
        if ctrl && !shift && !alt && code == KeyCode::KeyK {
            self.open_palette();
            return true;
        }
        if ctrl && !shift && !alt && code == KeyCode::KeyP {
            self.open_palette_with(">");
            return true;
        }
        if ctrl && !shift && !alt && code == KeyCode::Comma {
            self.open_settings();
            return true;
        }
        if ctrl && !shift && !alt && code == KeyCode::KeyF {
            self.open_find();
            return true;
        }
        if ctrl && !shift && !alt && code == KeyCode::KeyW {
            self.close_pane(event_loop);
            return true;
        }
        if ctrl && !shift && !alt && code == KeyCode::KeyT {
            self.new_pane();
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
        if self.settings.is_some() {
            if event.state == ElementState::Pressed {
                self.settings_key(&event);
            }
            return;
        }
        if self.palette.is_some() {
            if event.state == ElementState::Pressed {
                self.palette_key(&event, event_loop);
            }
            return;
        }
        if self.copy_mode.is_some() && self.palette.is_none() && self.settings.is_none() {
            if event.state == ElementState::Pressed {
                self.copy_mode_key(&event);
            }
            return;
        }
        if self.find.is_some() && self.palette.is_none() && self.settings.is_none() {
            if event.state == ElementState::Pressed {
                self.find_key(&event);
            }
            return;
        }
        if self.menu.is_some() && event.state == ElementState::Pressed {
            self.close_overlay();
            if code == KeyCode::Escape {
                return;
            }
        }
        if event.state == ElementState::Pressed && self.binding(code, event_loop) {
            return;
        }
        let Some(id) = self.focused() else { return };
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
        // An input method can drop the text of a key held with Alt; the
        // key's own character stands in for it, so Alt b is still Esc b.
        let logical = match &event.logical_key {
            winit::keyboard::Key::Character(s) => s.as_str(),
            _ => "",
        };
        let text = event.text.as_deref().filter(|t| !t.is_empty()).unwrap_or(if m.alt_key() { logical } else { "" });
        // Broadcasting, every pane in the workspace gets the key, each as
        // its own program would have it.
        for target in self.input_targets(id) {
            let Some(p) = self.panes.get_mut(&target) else { continue };
            let bytes = p.term.encode_key(crate::keys::ghostty(code), mods, action, text, unshifted);
            if crate::debug() && event.state == ElementState::Pressed && target == id {
                eprintln!("key {code:?} mods {m:?} text {:?} logical {logical:?} unshifted {unshifted:#x} -> {bytes:?}", event.text.as_deref());
            }
            if !bytes.is_empty() {
                p.pty.write(&bytes);
                if !p.term.at_bottom() {
                    p.term.scroll_to_bottom();
                    self.full = true;
                }
            }
        }
        self.redraw();
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

    fn mouse_press(&mut self, button: MouseButton, event_loop: &ActiveEventLoop) {
        if self.settings_click() || self.overlay_click(button, event_loop) || (button == MouseButton::Left && self.find_click()) {
            return;
        }
        let (x, y) = self.pointer;
        if button == MouseButton::Left && self.mods.control_key() && self.open_link() {
            return;
        }
        if let (Some((id, _)), Some(b)) = (self.pane_at(x, y), App::button_of(button)) {
            if self.mouse_to_program(id) && !self.mods.control_key() && self.caption_at(x, y).is_none() {
                if !self.roam_shown {
                    self.set_focus(id);
                }
                self.mouse_held = Some((id, b));
                self.send_mouse(id, sys::GHOSTTY_MOUSE_ACTION_PRESS, Some(b));
                return;
            }
        }
        if button == MouseButton::Right {
            if let Some((id, _)) = self.pane_at(x, y) {
                if !self.roam_shown {
                    self.set_focus(id);
                }
            }
            self.open_menu();
            return;
        }
        if button == MouseButton::Left {
            if let Some(pane) = self.caption_at(x, y) {
                self.drag = Some(Drag::Pane(dnd::Carry { pane, from: (x, y), moving: false, over: None }));
                return;
            }
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
                    self.copy(false);
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
        if self.find_hover(x, y) || self.settings_hover(x, y) || self.overlay_hover(x, y) {
            if let Some(w) = &self.window {
                w.set_cursor(CursorIcon::Default);
            }
            return;
        }
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
            Some(Drag::Pane(_)) => self.carry_move(x, y),
            None if self.mouse_held.is_some() => {
                let (id, b) = self.mouse_held.unwrap();
                self.send_mouse(id, sys::GHOSTTY_MOUSE_ACTION_MOTION, Some(b));
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
                self.update_link();
                if self.link.is_some() {
                    return;
                }
                // A program that follows every move of the mouse is told of it.
                if let Some((id, _)) = self.pane_at(x, y).filter(|(id, _)| self.mouse_to_program(*id)) {
                    self.send_mouse(id, sys::GHOSTTY_MOUSE_ACTION_MOTION, None);
                }
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
        if let Some((id, b)) = self.mouse_held.take() {
            self.send_mouse(id, sys::GHOSTTY_MOUSE_ACTION_RELEASE, Some(b));
            return;
        }
        if let Some(Drag::Pane(c)) = self.drag {
            self.ghost = None;
            self.drag = None;
            self.carry_drop(c);
            self.full = true;
            return self.redraw();
        }
        self.settings_release();
        if let Some(Drag::Select { .. }) = self.drag.take() {
            // What is selected is copied, to the clipboard and to the
            // selection a middle click pastes.
            self.copy(true);
            self.copy(false);
        }
    }

    fn scroll(&mut self, lines: isize) {
        let (x, y) = self.pointer;
        let Some((id, _)) = self.pane_at(x, y) else { return };
        if self.mouse_to_program(id) {
            let wheel = if lines < 0 { sys::GHOSTTY_MOUSE_BUTTON_FOUR } else { sys::GHOSTTY_MOUSE_BUTTON_FIVE };
            // One wheel event a notch, a notch being three lines.
            for _ in 0..lines.unsigned_abs().div_ceil(3).min(10) {
                self.send_mouse(id, sys::GHOSTTY_MOUSE_ACTION_PRESS, Some(wheel));
            }
            return;
        }
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
        let searched = self.find.as_ref().map(|f| f.pane);
        let mut search_again = false;
        while let Ok(out) = self.rx.try_recv() {
            match out {
                Output::Bytes(id, bytes) => {
                    if let Some(p) = self.panes.get_mut(&id) {
                        p.term.write(&bytes);
                        search_again |= searched == Some(id);
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
        if let (true, Some(id)) = (search_again, searched) {
            self.find_output(id);
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

    /// The accent and theme now, and every pane in its workspace's theme.
    pub(super) fn retint(&mut self) {
        let ws = self.workspaces.get(self.current);
        self.cur_theme = ws.and_then(|w| w.theme.as_deref()).map(Theme::by_name).unwrap_or(self.theme);
        self.cur_accent = ws.and_then(|w| w.accent.as_deref()).map(crate::theme::accent).unwrap_or(self.accent);
        let mut wanted: Vec<(PaneId, &'static Theme)> = Vec::new();
        for ws in &self.workspaces {
            let t = ws.theme.as_deref().map(Theme::by_name).unwrap_or(self.theme);
            let mut ids = Vec::new();
            ws.root.panes(&mut ids);
            wanted.extend(ids.into_iter().map(|id| (id, t)));
        }
        wanted.extend(self.roam.map(|r| (r, self.theme)));
        for (id, t) in wanted {
            if let Some(p) = self.panes.get_mut(&id) {
                if !std::ptr::eq(p.theme, t) {
                    p.term.set_theme(t);
                    p.theme = t;
                    p.full = true;
                    self.full = true;
                }
            }
        }
    }

    fn draw(&mut self) {
        self.retint();
        self.refresh_captions();
        let full = std::mem::take(&mut self.full);
        if full {
            self.canvas.fill(Rect { x: 0, y: 0, w: self.canvas.width, h: self.canvas.height }, self.cur_theme.divider());
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
        // Under the palette, the menu or the roaming terminal, panes are
        // drawn only when all of it is, so that what they write does not
        // land on top.
        let floating = self.palette.is_some() || self.menu.is_some() || self.settings.is_some();
        if full || !(self.roam_shown || floating) {
            for (n, id) in ids.iter().enumerate() {
                self.draw_pane(*id, n + 1, full, Some(*id) == focus && !self.roam_shown);
            }
        }
        if self.roam_shown && (full || !floating) {
            if let Some(r) = self.roam {
                if full {
                    let rect = self.panes[&r].rect;
                    let b = self.px(2.0);
                    self.canvas.fill(Rect { x: rect.x - b, y: rect.y - b, w: rect.w + 2 * b, h: rect.h + 2 * b }, ROAM_BG);
                }
                self.draw_pane(r, 0, full, true);
            }
        }
        if full && self.config.status_line {
            self.draw_status();
        }
        if full {
            self.draw_carry();
        }
        if self.find.is_some() && (full || !floating) {
            self.draw_find();
        }
        if full {
            self.draw_palette();
            self.draw_menu();
            self.draw_settings();
        }
        self.present();
    }

    fn draw_pane(&mut self, id: PaneId, n: usize, full: bool, focused: bool) {
        let (cw, ch) = (self.fonts.cell_w, self.fonts.cell_h);
        let cap_h = self.caption_h();
        let (sel, tfg) = (self.cur_theme.selection, self.cur_theme.fg);
        let min_contrast = self.config.min_contrast;
        let (rows, cols) = self.panes.get(&id).map_or((0, 0), |p| (p.rows, p.cols));
        let (hl_all, hl_pick) = self.find_spans(id, rows, cols);
        let (link_sp, link_color) = (self.link_spans(id), self.cur_accent);
        let copy_cur = self.copy_cursor(id);
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
        // With pictures in it, the pane is drawn whole, pictures over text.
        let mut placements = p.term.placements();
        placements.sort_by_key(|pl| pl.z);
        let canvas = &mut self.canvas;
        let fonts = &mut self.fonts;
        let damage = &mut self.damage;
        let cols_n = p.cols as usize;
        let lines = &mut p.lines;
        if lines.len() != p.rows as usize {
            lines.resize(p.rows as usize, Vec::new());
        }
        p.term.rows(full || dirty == 2 || !placements.is_empty(), &extra, &mut row, |y, cells: &[Cell]| {
            // The row's characters, one a column, for finding links in.
            if let Some(l) = lines.get_mut(y as usize) {
                l.clear();
                l.extend(cells.iter().take(cols_n).map(|c| c.text.chars().next().unwrap_or(' ')));
            }
            let top = content.y + y as i32 * ch;
            if top + ch > content.y + content.h + ch {
                return;
            }
            // The row across the whole pane, so its edges match.
            let line = Rect { x: rect.x, y: top, w: rect.w, h: ch };
            canvas.fill(line, dbg);
            let clip = Rect { x: rect.x, y: top, w: rect.w, h: ch };
            let colors = |c: &Cell, x: u16, y: u16| {
                let (mut fg, mut bg) = (c.fg.unwrap_or(dfg), c.bg.unwrap_or(dbg));
                if c.flags & vt::INVERSE != 0 {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if c.selected {
                    bg = sel;
                }
                if c.flags & vt::FAINT != 0 {
                    fg = mix(bg, fg, 150);
                }
                let (fg, bg) = App::match_colors(&hl_all, &hl_pick, x, y, fg, bg);
                (crate::canvas::readable(fg, bg, min_contrast), bg)
            };
            for (x, c) in cells.iter().enumerate() {
                let (_, bg) = colors(c, x as u16, y);
                if bg != dbg {
                    canvas.fill(Rect { x: content.x + x as i32 * cw, y: top, w: cw, h: ch }, bg);
                }
            }
            // Ligatures: each run of symbols, as => or !=, shaped, and the
            // cells whose glyph that changes drawn with the shaped one.
            let mut ligs: Vec<Option<u16>> = vec![None; cells.len()];
            if fonts.ligatures {
                let mut i = 0;
                while i < cells.len() {
                    let one = |c: &Cell| c.text.chars().count() == 1 && !c.text.starts_with(' ') && c.text.is_ascii();
                    if !one(&cells[i]) {
                        i += 1;
                        continue;
                    }
                    let bold = cells[i].flags & vt::BOLD != 0;
                    let mut j = i;
                    while j < cells.len() && one(&cells[j]) && (cells[j].flags & vt::BOLD != 0) == bold {
                        j += 1;
                    }
                    let run: String = cells[i..j].iter().map(|c| c.text.as_str()).collect();
                    if j - i >= 2 && run.chars().any(|ch| "=<>!-|&*/:.+~#_%^?;\\\\[]{}()w".contains(ch)) {
                        if let Some(ids) = fonts.shape(&run, bold) {
                            for (k, (id, ch)) in ids.iter().zip(run.chars()).enumerate() {
                                if *id != fonts.plain_id(ch, bold) {
                                    ligs[i + k] = Some(*id);
                                }
                            }
                        }
                    }
                    i = j;
                }
            }
            for (x, c) in cells.iter().enumerate() {
                let left = content.x + x as i32 * cw;
                let (mut fg, bg) = colors(c, x as u16, y);
                let on_cursor = cursor.visible && focused && cursor.y == y && cursor.x as usize == x && cursor.style == sys::GHOSTTY_CURSOR_STYLE_BLOCK;
                if on_cursor {
                    canvas.fill(Rect { x: left, y: top, w: cw, h: ch }, cursor.color.unwrap_or(tfg));
                    fg = bg;
                }
                if c.flags & vt::INVISIBLE == 0 {
                    if let Some(chr) = c.text.chars().next() {
                        if chr != ' ' {
                            // An emoji is two cells wide unless the next has text in it.
                            let narrow = cells.get(x + 1).is_some_and(|n| !n.text.is_empty()) || x + 1 == cells.len();
                            let style = (c.flags & vt::BOLD != 0) as u8 | (((c.flags & vt::ITALIC != 0) as u8) << 1) | ((narrow as u8) << 2);
                            let g = match ligs.get(x).copied().flatten() {
                                Some(id) => fonts.glyph_id(id, style),
                                None => fonts.glyph(chr, style),
                            };
                            canvas.glyph(g, left, top, fg, clip);
                        }
                    }
                }
                if link_sp.iter().any(|s| s.y == y && (x as u16) >= s.x0 && (x as u16) <= s.x1) {
                    canvas.fill(Rect { x: left, y: top + ch - (ch / 10).max(2), w: cw, h: (ch / 16).max(1) }, link_color);
                } else if c.underline() {
                    canvas.fill(Rect { x: left, y: top + ch - (ch / 10).max(2), w: cw, h: (ch / 16).max(1) }, fg);
                }
                if c.flags & vt::STRIKE != 0 {
                    canvas.fill(Rect { x: left, y: top + ch / 2, w: cw, h: (ch / 16).max(1) }, fg);
                }
            }
            if let Some((cx, _)) = copy_cur.filter(|(_, cy)| *cy == y) {
                let left = content.x + cx as i32 * cw;
                canvas.fill(Rect { x: left, y: top, w: cw, h: 2 }, link_color);
                canvas.fill(Rect { x: left, y: top + ch - 2, w: cw, h: 2 }, link_color);
                canvas.fill(Rect { x: left, y: top, w: 2, h: ch }, link_color);
                canvas.fill(Rect { x: left + cw - 2, y: top, w: 2, h: ch }, link_color);
            }
            if cursor.visible && cursor.y == y && copy_cur.is_none() {
                let left = content.x + cursor.x as i32 * cw;
                let color = cursor.color.unwrap_or(tfg);
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
        // Pictures, over the text, each scaled into the cells it covers.
        for pl in &placements {
            let key = (id, pl.image);
            let stale = self.pictures.get(&key).is_none_or(|(g, _)| *g != pl.generation);
            if stale {
                if let Some(pic) = p.term.picture(pl.image) {
                    self.pictures.insert(key, (pl.generation, std::rc::Rc::new(pic)));
                }
            }
            let Some((_, pic)) = self.pictures.get(&key).cloned() else { continue };
            let dest = Rect { x: content.x + pl.col * cw + pl.offset.0 as i32, y: content.y + pl.row * ch + pl.offset.1 as i32, w: pl.size.0 as i32, h: pl.size.1 as i32 };
            let src = if pl.source.2 == 0 { (0, 0, pic.width, pic.height) } else { pl.source };
            self.canvas.picture(&pic.pixels, pic.width, src, dest, content);
            self.damage.push(dest);
        }
        // The caption, along the bottom of the pane, as screen's is.
        let caption = p.caption.clone();
        let scrolled = !p.term.at_bottom();
        let activity = p.activity;
        if full {
            if self.config.captions {
                self.draw_caption(rect, n, &caption, focused, scrolled, activity);
            }
        }
    }

    fn draw_caption(&mut self, rect: Rect, n: usize, caption: &str, focused: bool, scrolled: bool, activity: bool) {
        let h = self.caption_h();
        let bar = Rect { x: rect.x, y: rect.y + rect.h - h, w: rect.w, h };
        let (bg, fg) = if focused {
            (self.cur_accent, crate::canvas::on(self.cur_accent))
        } else {
            let (bg, fg) = self.cur_theme.idle_caption();
            (bg, if activity { ACTIVITY } else { fg })
        };
        self.canvas.fill(bar, bg);
        let ty = bar.y + (h - self.fonts.ui.height) / 2;
        let pad = self.px(6.0) as f32;
        let label = if n == 0 { format!("~  {caption}") } else { format!("{n}  {caption}") };
        self.canvas.text(&mut self.fonts.ui, &label, bar.x as f32 + pad, ty, fg, bar);
        // Tags at the right: scrolled back, and typing into every pane.
        let broadcast = n > 0 && self.workspaces.get(self.current).is_some_and(|w| w.broadcast);
        let mut right = bar.x + bar.w - pad as i32;
        let copying = self.copy_mode.as_ref().is_some_and(|c| self.panes.get(&c.pane).is_some_and(|p| p.rect == rect));
        for (on, tag, color) in [(copying, " COPY ", 0xFFC53D), (scrolled && !copying, " SCROLLED ", SCROLLED), (broadcast, " TYPING INTO ALL ", 0xE5484D)] {
            if !on {
                continue;
            }
            let w = self.fonts.ui.width(tag);
            let r = Rect { x: right - w, y: bar.y, w, h };
            self.canvas.fill(r, color);
            self.canvas.text(&mut self.fonts.ui, tag, r.x as f32, ty, crate::canvas::on(color), r);
            right = r.x - pad as i32 / 2;
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
            .map(|ws| WorkspaceSpec { name: ws.name.clone(), layout: Some(persist::spec(&ws.root, &describe)), accent: ws.accent.clone(), theme: ws.theme.clone(), ..Default::default() })
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
        self.fonts = Fonts::new(&self.font_choice(), self.scale);
        let size = window.inner_size();
        self.canvas.resize(size.width as i32, size.height as i32);
        if let Some(s) = self.surface.as_mut() {
            let _ = s.resize(NonZeroU32::new(size.width.max(1)).unwrap(), NonZeroU32::new(size.height.max(1)).unwrap());
        }
        self.window = Some(window);
        self.start();
        self.load_recent();
        self.set_title();
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
            WindowEvent::ModifiersChanged(m) => {
                self.mods = m.state();
                self.update_link();
            }
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => self.key(event, event_loop),
            WindowEvent::CursorMoved { position, .. } => self.mouse_move(position.x as i32, position.y as i32),
            WindowEvent::MouseInput { state: ElementState::Pressed, button, .. } => self.mouse_press(button, event_loop),
            WindowEvent::MouseInput { state: ElementState::Released, .. } => self.mouse_release(),
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -(y * 3.0) as isize,
                    MouseScrollDelta::PixelDelta(p) => -(p.y / self.fonts.cell_h as f64) as isize,
                };
                if lines != 0 && !self.settings_scroll(lines) {
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
