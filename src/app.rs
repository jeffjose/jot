//! The window: workspaces of panes, each pane a program on a terminal of
//! its own, with screen's keys to split, move between and close them.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
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
mod automate;
mod bar;
mod commands;
mod copymode;
mod editor;
mod info;
mod dnd;
mod find;
mod links;
mod minimize;
mod pin;
mod mouse;
mod popout;
mod restore;
mod settings;
mod slash;
mod space;
mod toast;
mod watch;

// Colors from Radix, as the screen setup has them.
const STATUS_BG: u32 = 0x111111;
const STATUS_FG: u32 = 0xB4B4B4;
const CURRENT_BG: u32 = 0x30A46C; // green 9
const CURRENT_FG: u32 = 0x0E1512;
const BADGE_BG: u32 = 0x8EC8F6; // blue 7
const BADGE_FG: u32 = 0x113264;
const ACTIVITY: u32 = 0xFFCA16; // amber
const BELL: u32 = 0xFF9592; // red 11
const ROAM_BG: u32 = 0x6E56CF; // violet 9
/// How long the window must keep a size before the GPU is set up for it;
/// until then frames go up the CPU's way, as jog does with its videos.
const RESIZE_SETTLE: Duration = Duration::from_millis(200);

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
    /// Where the cursor was drawn, column and row, if it was.
    cursor_was: Option<(u16, u16)>,
    /// The theme its terminal has: its workspace's, or the settings'.
    theme: &'static Theme,
    caption: String,
    /// Wrote something while its workspace was not shown.
    activity: bool,
    /// When it started: what it prints at first, its prompt, is not news.
    started: Instant,
    /// The characters of each row as last drawn, to find links in.
    lines: Vec<Vec<char>>,
    /// What the shell's marks have told of its commands.
    shell: commands::Shell,
    /// A picture has been sent to it (kitty graphics): only then are its
    /// pictures looked for, each frame.
    has_images: bool,
    /// Text to be printed into its terminal, by jot, as /info is.
    print: Option<info::Print>,
    /// Its output written to a file too (/tee), and its command's lines
    /// filtered (/filter).
    tee: Option<watch::Tee>,
    filter: Option<watch::Filter>,
    /// The last line it printed, for its row when minimized.
    last_line: String,
    /// Commands pinned to its top, and how far their strip is scrolled.
    pins: Vec<pin::Pin>,
    pins_scroll: usize,
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
    on_open: Option<String>,
    on_focus: Option<String>,
    /// What is typed goes to every pane in it.
    broadcast: bool,
    /// Panes clicked out of what is typed into, while broadcasting.
    left_out: Vec<PaneId>,
    /// Panes shrunk to a row at the top, out of the layout but running;
    /// each with where it was, to go back there.
    minimized: Vec<PaneId>,
    /// Panes made into empty space: room only, no shell, no focus.
    spaces: Vec<PaneId>,
}

enum Drag {
    Divider(Divider),
    Select { pane: PaneId, from: (u16, u16) },
    Pane(dnd::Carry),
}

pub struct App {
    config: Config,
    proxy: EventLoopProxy<()>,
    rx: Receiver<Output>,
    /// Set while a wake is on its way, so a burst of output wakes the
    /// window once.
    woken: Arc<AtomicBool>,
    /// The GPU the main window is drawn with, when there is one: before
    /// the window, so that it goes first.
    gpu: Option<crate::gpu::Gpu>,
    window: Option<Rc<Window>>,
    surface: Option<Surface<Rc<Window>, Rc<Window>>>,
    canvas: Canvas,
    fonts: Fonts,
    font_size: f32,
    scale: f32,
    theme: &'static Theme,
    /// The color of what is picked out: the pane you are in, menus.
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
    /// The size the window was last given, taken up when the next frame
    /// is drawn: a drag at the window's edge sends sizes faster than
    /// frames go up, and each one taken up costs the GPU a new swapchain
    /// and every terminal a reflow, for a size never shown.
    resized: Option<(u32, u32)>,
    /// While the window is being resized, when the GPU is next set up for
    /// its size: a new swapchain for each size of a drag stalls each frame
    /// for several milliseconds, where the CPU puts the frame up in one.
    gpu_settle: Option<Instant>,
    damage: Vec<Rect>,
    mods: ModifiersState,
    pointer: (i32, i32),
    drag: Option<Drag>,
    /// The pane whose corner handle the pointer is over.
    handle_hover: Option<PaneId>,
    /// Slash commands being typed at a prompt.
    slash: Option<slash::Slash>,
    /// A pane's own command line, open, and where its buttons are.
    bar: Option<bar::Bar>,
    bar_hits: Vec<(Rect, PaneId, bar::BarHit)>,
    /// Which of them the pointer is on, to light it.
    bar_lit: Option<usize>,
    /// Where an open bar's slash commands rise from.
    bar_list_at: Option<(i32, i32)>,
    /// Where an open bar's text field is, and where its text starts;
    /// whether a drag in it is selecting; its last clicks, for double
    /// and triple ones.
    bar_field: Option<(Rect, i32)>,
    bar_drag: bool,
    bar_clicks: Option<(Instant, u8)>,
    /// The MULTI tags on captions, to click, and the one the pointer is on.
    multi_tags: Vec<(Rect, PaneId, watch::Tag)>,
    multi_lit: Option<(PaneId, watch::Tag)>,
    /// Minimized panes' rows, to click, and the one the pointer is on.
    mini_hits: Vec<(Rect, PaneId)>,
    mini_lit: Option<PaneId>,
    /// Where the current workspace's minimized rows are, as laid out.
    mini_rows: Vec<(PaneId, Rect)>,
    /// Pin strips' buttons, and the one the pointer is on.
    pin_hits: Vec<(Rect, PaneId, usize, pin::PinHit)>,
    pin_lit: Option<(PaneId, usize, pin::PinHit)>,
    /// Commands run now from a bar, as they end: pane, output, status.
    ran: Arc<std::sync::Mutex<Vec<(PaneId, String, i32)>>>,
    last_click: Option<(Instant, (u16, u16), u8)>,
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
    /// When the panes' scrollback was last kept.
    scrollback_at: Instant,
    /// Panes to put in windows of their own once the main one is up.
    pending_popouts: Vec<persist::Spec>,
    /// When the wheel last turned, and how many quick turns in a row.
    wheel_at: Instant,
    wheel_streak: u32,
    toasts: Vec<toast::Toast>,
    /// The last session, offered back rather than brought back.
    offered: Option<restore::Offer>,
    /// The next start may offer the last session (only the first may).
    may_offer: bool,
    /// Pictures' pixels, by pane and picture, with the generation read.
    pictures: HashMap<(PaneId, u32), (u64, std::rc::Rc<vt::Picture>)>,
    triggers: Vec<automate::Trigger>,
    /// When each trigger last went off, in each pane.
    fired: HashMap<(usize, PaneId), Instant>,
    /// What the `jot` command asks.
    requests: Receiver<crate::remote::Request>,
    /// Panes in windows of their own, and the one with the keyboard.
    popouts: Vec<popout::Popout>,
    focused_popout: Option<WindowId>,
    /// The main window has the focus: what happens in it is seen.
    focused_window: bool,
    editor: Option<commands::Editor>,
    /// Where the right-click menu was opened.
    menu_at: (i32, i32),
    /// What was run from the palette lately, the latest first.
    recent: Vec<String>,
    /// Panes closed, the last last, for Ctrl Shift T.
    closed: Vec<Closed>,
    /// What jot last put on the clipboard and the selection.
    copied: (Option<String>, Option<String>),
    saved_at: Instant,
    unsaved: bool,
    captions_at: Instant,
    clock: String,
    session: Option<Session>,
    /// The session the panes run in.
    mux: Arc<crate::mux::Client>,
    /// Panes the session had when this window took it up, until the
    /// layout takes them up.
    held: HashMap<u64, crate::mux::Held>,
    /// Panes taken up whose output is yet to be replayed.
    replays: Vec<(PaneId, Vec<u8>)>,
    replaying: bool,
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
        // The `jot` command's requests, each waking the window.
        let (req_tx, requests) = channel();
        let (wake, woke) = (proxy.clone(), Arc::new(AtomicBool::new(false)));
        let _ = woke;
        crate::remote::listen(move |r| {
            let _ = req_tx.send(r);
            let _ = wake.send_event(());
        });
        // The session the panes run in: one no window has, or a new one.
        let woken = Arc::new(AtomicBool::new(false));
        let attached = {
            let (tx, proxy, woken) = (tx.clone(), proxy.clone(), woken.clone());
            crate::mux::Client::open(move |heard| {
                let out = match heard {
                    crate::mux::Heard::Output(id, b) => Output::Bytes(id, b),
                    crate::mux::Heard::Exit(id) => Output::Exit(id),
                    crate::mux::Heard::Mode(id, raw) => Output::Mode(id, raw),
                    crate::mux::Heard::Gone => Output::Gone,
                };
                let _ = tx.send(out);
                if !woken.swap(true, Ordering::AcqRel) {
                    let _ = proxy.send_event(());
                }
            })
        };
        let attached = match attached {
            Ok(a) => a,
            Err(e) => {
                eprintln!("jot: could not start a session: {e}");
                std::process::exit(1);
            }
        };
        // Taking up a session that has panes: its layout, not the last
        // one saved, and nothing offered.
        let taking_up = !attached.panes.is_empty();
        let session = if taking_up { toml::from_str::<Session>(&attached.layout).ok() } else { session };
        let next_id = attached.panes.iter().map(|h| h.pane + 1).max().unwrap_or(1);
        let held: HashMap<u64, crate::mux::Held> = attached.panes.into_iter().map(|h| (h.pane, h)).collect();
        let font_size = config.font_size;
        let theme = Theme::by_name(&config.theme);
        let accent = crate::theme::accent(&config.accent);
        App {
            fonts: Fonts::new(&crate::font::Choice { family: config.font_family.clone(), size: font_size, ui_family: config.ui_font_family.clone(), ui_size: config.ui_font_size, line_height: config.line_height, ligatures: config.ligatures, bold: config.bold_text, lcd: crate::font::lcd_for(&config.subpixel) }, 1.0),
            font_size,
            config,
            proxy,
            rx,
            woken,
            gpu: None,
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
            next_id,
            dividers: Vec::new(),
            full: true,
            resized: None,
            gpu_settle: None,
            damage: Vec::new(),
            mods: ModifiersState::empty(),
            pointer: (0, 0),
            drag: None,
            handle_hover: None,
            slash: None,
            bar: None,
            bar_hits: Vec::new(),
            bar_lit: None,
            bar_list_at: None,
            bar_field: None,
            bar_drag: false,
            bar_clicks: None,
            multi_tags: Vec::new(),
            multi_lit: None,
            mini_hits: Vec::new(),
            mini_lit: None,
            mini_rows: Vec::new(),
            pin_hits: Vec::new(),
            pin_lit: None,
            ran: Default::default(),
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
            scrollback_at: Instant::now(),
            pending_popouts: Vec::new(),
            wheel_at: Instant::now(),
            wheel_streak: 0,
            toasts: Vec::new(),
            offered: None,
            may_offer: !taking_up,
            pictures: HashMap::new(),
            triggers: Vec::new(),
            fired: HashMap::new(),
            requests,
            popouts: Vec::new(),
            focused_popout: None,
            focused_window: true,
            editor: None,
            menu_at: (0, 0),
            recent: Vec::new(),
            closed: Vec::new(),
            copied: (None, None),
            saved_at: Instant::now(),
            unsaved: false,
            captions_at: Instant::now() - Duration::from_secs(10),
            clock: clock(),
            session,
            mux: attached.client,
            held,
            replays: Vec::new(),
            replaying: false,
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

    /// Starts a pane: its program in the session, and the terminal it
    /// writes to. A pane the session already has is taken up instead, its
    /// output to be replayed.
    fn spawn(&mut self, start: Start) -> PaneId {
        let (cols, rows) = (80, 24);
        let held = start.pane.and_then(|p| self.held.remove(&p));
        let (id, pid, cmd) = match held {
            Some(h) => {
                self.replays.push((h.pane, h.log));
                (h.pane, h.pid, h.cmd)
            }
            None => {
                let id = self.next_id;
                self.next_id += 1;
                match self.mux.spawn(id, &start.cmd, &start.cwd, cols, rows, (0, 0)) {
                    Ok(pid) => (id, pid, start.cmd),
                    Err(e) => {
                        eprintln!("jot: could not start a shell: {e}");
                        std::process::exit(1);
                    }
                }
            }
        };
        let pty = Pty { pane: id, pid, mux: self.mux.clone(), raw: false };
        let mux = self.mux.clone();
        let reply: vt::Reply = Box::new(move |bytes: &[u8]| {
            if !mux.mute.load(Ordering::Acquire) {
                mux.input(id, bytes);
            }
        });
        let term = Terminal::new(cols, rows, self.config.scrollback, self.theme, reply);
        let caption = folder_name(&pty.cwd().unwrap_or(start.cwd));
        self.panes.insert(
            id,
            Pane { term, pty, cmd, content: Rect::default(), rect: Rect::default(), cols, rows, full: true, cursor_was: None, theme: self.theme, caption, activity: false, started: Instant::now(), lines: Vec::new(), shell: commands::Shell::default(), has_images: false, print: None, tee: None, filter: None, last_line: String::new(), pins: Vec::new(), pins_scroll: 0 },
        );
        id
    }

    /// Panes taken up from the session: what they printed, replayed into
    /// their terminals, at the sizes it was printed at, then their own.
    fn replay_held(&mut self) {
        if self.replays.is_empty() || self.workspaces.is_empty() {
            return;
        }
        self.replaying = true;
        self.mux.mute.store(true, Ordering::Release);
        for (id, log) in std::mem::take(&mut self.replays) {
            for e in crate::mux::log_entries(&log) {
                let Some(p) = self.panes.get_mut(&id) else { break };
                match e {
                    crate::mux::Entry::Size(cols, rows) => {
                        let (cw, ch) = (self.fonts.cell_w as u32, self.fonts.cell_h as u32);
                        p.term.resize(cols, rows, cw, ch);
                        p.cols = cols;
                        p.rows = rows;
                    }
                    crate::mux::Entry::Bytes(b) => {
                        p.term.write(b);
                        if !p.has_images && b.windows(3).any(|w| w == b"\x1b_G") {
                            p.has_images = true;
                        }
                        self.shell_events(id);
                    }
                }
            }
            if let Some(p) = self.panes.get_mut(&id) {
                p.full = true;
            }
        }
        self.mux.mute.store(false, Ordering::Release);
        self.replaying = false;
        // Back to the sizes the window gives them.
        self.relayout();
    }

    fn add_workspace(&mut self, spec: &WorkspaceSpec) {
        let mut ids = Vec::new();
        let root = spec.build(&mut |start| {
            let id = self.spawn(start);
            ids.push(id);
            id
        });
        // The pins each pane had when the session was saved.
        if let Some(layout) = &spec.layout {
            for (id, pins) in ids.iter().zip(persist::pins_in_order(layout)) {
                if let Some(p) = self.panes.get_mut(id) {
                    p.pins = pins.into_iter().map(|cmd| pin::Pin { cmd, last: None }).collect();
                }
            }
        }
        // And which were empty space: their shells ended, the space kept.
        let mut spaces = Vec::new();
        if let Some(layout) = &spec.layout {
            for (id, empty) in ids.iter().zip(persist::empty_in_order(layout)) {
                if empty {
                    if let Some(p) = self.panes.get(id) {
                        unsafe { libc::kill(-p.pty.pid, libc::SIGHUP) };
                    }
                    spaces.push(*id);
                }
            }
        }
        let focus = ids.iter().copied().find(|i| !spaces.contains(i)).unwrap_or(ids[0]);
        let name = if spec.name.is_empty() { folder_name(&spec.folder()) } else { spec.name.clone() };
        self.workspaces.push(Workspace { name, root, focus, zoom: None, accent: spec.accent.clone(), theme: spec.theme.clone(), on_open: spec.on_open.clone(), on_focus: spec.on_focus.clone(), broadcast: false, left_out: Vec::new(), minimized: Vec::new(), spaces });
        self.run_hook(self.workspaces.len() - 1, true);
    }

    /// The workspaces to begin with: those saved, else those in the
    /// config, else one shell where jot was started.
    fn start(&mut self) {
        // Opening fresh, the last session offered on a card, unless the
        // settings say to bring it back always, or never.
        if std::mem::take(&mut self.may_offer) {
            match self.config.restore.as_str() {
                "always" => {}
                "never" => self.session = None,
                _ => {
                    if let Some(s) = self.session.take() {
                        self.offer(s);
                    }
                }
            }
        }
        let session = self.session.take();
        let taking_up = !self.held.is_empty();
        let restoring = session.is_some() && !taking_up;
        let (specs, current, roam) = match session {
            Some(s) => {
                self.pending_popouts = s.popouts;
                (s.workspaces, s.current, s.roam)
            }
            None if taking_up => (Vec::new(), 0, None),
            None if !self.config.workspaces.is_empty() => (std::mem::take(&mut self.config.workspaces), 0, None),
            None => {
                let cwd = std::env::current_dir().unwrap_or_else(|_| pty::home());
                (vec![WorkspaceSpec { name: String::new(), cwd: Some(cwd.display().to_string()), panes: vec![], split: None, layout: None, ..Default::default() }], 0, None)
            }
        };
        for spec in &specs {
            self.add_workspace(spec);
        }
        // Panes the session has that the layout did not take up (popped
        // out, roaming, or the layout lost): a workspace of their own.
        let popped: Vec<u64> = self.pending_popouts.iter().filter_map(|s| if let persist::Spec::Pane { id, .. } = s { *id } else { None }).collect();
        let mut left: Vec<u64> = self.held.keys().copied().filter(|p| !popped.contains(p)).collect();
        left.sort();
        if !left.is_empty() || self.workspaces.is_empty() {
            let panes = left.iter().map(|id| persist::Spec::Pane { cmd: String::new(), cwd: None, pins: Vec::new(), empty: false, id: Some(*id) }).collect::<Vec<_>>();
            let layout = match panes.len() {
                0 => None,
                1 => panes.into_iter().next(),
                _ => Some(persist::Spec::Across { v: panes, sizes: Vec::new() }),
            };
            self.add_workspace(&WorkspaceSpec { name: if taking_up { "Kept".into() } else { String::new() }, layout, ..Default::default() });
        }
        self.current = current.min(self.workspaces.len().saturating_sub(1));
        self.roam_cwd = roam.map(|r| persist::expand(&r));
        if restoring {
            for (wi, ws) in self.workspaces.iter().enumerate() {
                let mut ids = Vec::new();
                ws.root.panes(&mut ids);
                for (pi, id) in ids.iter().enumerate() {
                    restore_scrollback(self.panes.get_mut(id), &format!("{}-{}", wi + 1, pi + 1));
                }
            }
        }
        self.load_triggers();
        self.replay_held();
    }

    fn ws(&self) -> &Workspace {
        &self.workspaces[self.current]
    }

    fn ws_mut(&mut self) -> &mut Workspace {
        &mut self.workspaces[self.current]
    }

    /// The pane keys go to.
    fn focused(&self) -> Option<PaneId> {
        if let Some(po) = self.focused_popout.and_then(|w| self.popouts.iter().find(|p| p.window.id() == w)) {
            return Some(po.pane);
        }
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
        self.lay_out();
        self.redraw();
    }

    /// The window's new size taken up, if it has one: the frame, the
    /// GPU's swapchain and the layout made for it. Only from draw, so
    /// that the sizes a drag goes through between frames cost nothing.
    fn take_resize(&mut self) {
        let Some((width, height)) = self.resized.take() else { return };
        if (width.max(1) as i32, height.max(1) as i32) == (self.canvas.width, self.canvas.height) {
            return;
        }
        if let Some(s) = self.surface.as_mut() {
            let _ = s.resize(NonZeroU32::new(width.max(1)).unwrap(), NonZeroU32::new(height.max(1)).unwrap());
        }
        self.canvas.resize(width as i32, height as i32);
        // The GPU is set up for the size once it holds (in about_to_wait).
        if self.gpu.is_some() {
            self.gpu_settle = Some(Instant::now() + RESIZE_SETTLE);
        }
        // Not relayout: a redraw asked for while drawing is one frame more.
        self.lay_out();
    }

    /// The layout, without asking for a frame.
    fn lay_out(&mut self) {
        // Panes drawn again whole after this, their tags with them; none
        // left behind for a pane no longer on screen.
        self.multi_tags.clear();
        self.bar_hits.clear();
        self.pin_hits.clear();
        self.lay_out_popouts();
        let area = self.area();
        let mut shown = Vec::new();
        let row_h = self.mini_row_h();
        for (i, ws) in self.workspaces.iter().enumerate() {
            let (mut panes, mut dividers, mut rows) = (Vec::new(), Vec::new(), Vec::new());
            match ws.zoom {
                Some(z) => panes.push((z, area)),
                None => ws.root.layout_with(area, &ws.minimized, row_h, &mut panes, &mut rows, &mut dividers),
            }
            if i == self.current {
                self.dividers = dividers;
                self.mini_rows = rows;
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
            let bar = self.bar_height(id);
            let pins = self.pins_height(id);
            let Some(p) = self.panes.get_mut(&id) else { continue };
            let content = Rect { x: rect.x + pad, y: rect.y + top + pins, w: (rect.w - 2 * pad).max(0), h: (rect.h - cap - top - bar - pins).max(0) };
            let cols = (content.w / cw).clamp(1, u16::MAX as i32) as u16;
            let rows = (content.h / ch).clamp(1, u16::MAX as i32) as u16;
            // Only a change of rows or columns is told to the program: a
            // shell such as tcsh draws its prompt again on each telling.
            if (cols, rows) != (p.cols, p.rows) {
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
        self.split_side(dir, false);
    }

    /// A new shell beside the pane you are in: after it (right, down) or
    /// before it (left, up), in its folder.
    fn split_side(&mut self, dir: Dir, before: bool) {
        let at = self.focused().unwrap_or(0);
        let cwd = self.panes.get(&at).and_then(|p| p.pty.cwd()).unwrap_or_else(pty::home);
        self.split_at(dir, before, Start { cmd: String::new(), cwd, pane: None });
    }

    /// Splits the pane you are in, the new one running `start`.
    pub(super) fn split_with(&mut self, dir: Dir, start: Start) {
        self.split_at(dir, false, start);
    }

    fn split_at(&mut self, dir: Dir, before: bool, start: Start) {
        if self.roam_shown {
            return;
        }
        let at = self.ws().focus;
        let id = self.spawn(start);
        let ws = self.ws_mut();
        ws.zoom = None;
        ws.root.split_beside(at, id, dir, before);
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
        let id = self.spawn(Start { cmd: c.cmd, cwd: c.cwd, pane: None });
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
                self.workspaces.push(Workspace { name: c.workspace, root: Node::Pane(id), focus: id, zoom: None, accent: None, theme: None, on_open: None, on_focus: None, broadcast: false, left_out: Vec::new(), minimized: Vec::new(), spaces: Vec::new() });
                self.go_to(self.workspaces.len() - 1);
            }
        }
        self.roam_shown = false;
        self.changed();
    }

    /// Closes a pane, its workspace with it when it was the last; jot
    /// with the last workspace.
    fn close(&mut self, id: PaneId, event_loop: &ActiveEventLoop) {
        self.forget_minimized(id);
        // Commands waiting on it wait no more.
        self.release_after(id, false);
        // A pane in a window of its own takes the window with it.
        if let Some(i) = self.popout_of(id) {
            self.popouts.remove(i);
            self.focused_popout = None;
            self.panes.remove(&id);
            self.full = true;
            return self.redraw();
        }
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
                self.save_scrollback();
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
        // Minimized rows and empty space are not gone to.
        let (mut rows, row_h) = (Vec::new(), self.mini_row_h());
        self.ws().root.layout_with(self.area(), &self.ws().minimized, row_h, &mut panes, &mut rows, &mut divs);
        panes.retain(|(p, _)| !self.ws().spaces.contains(p));
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
            self.run_hook(i, false);
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
            self.roam = Some(self.spawn(Start { cmd: String::new(), cwd, pane: None }));
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
        crate::font::Choice { family: self.config.font_family.clone(), size: self.font_size, ui_family: self.config.ui_font_family.clone(), ui_size: self.config.ui_font_size, line_height: self.config.line_height, ligatures: self.config.ligatures, bold: self.config.bold_text, lcd: crate::font::lcd_for(&self.config.subpixel) }
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
    /// click pastes), and keeps it: reading back what jot itself put
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
    /// else what jot put there last.
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

    /// The panes what is typed goes to: the one you are in, or, typing
    /// into several, every pane of its workspace not clicked out.
    pub(super) fn input_targets(&self, id: PaneId) -> Vec<PaneId> {
        match self.workspaces.get(self.current) {
            Some(ws) if ws.broadcast && !self.roam_shown && ws.root.contains(id) => {
                let mut ids = Vec::new();
                ws.root.panes(&mut ids);
                ids.retain(|p| !ws.left_out.contains(p));
                if ids.is_empty() {
                    vec![id]
                } else {
                    ids
                }
            }
            _ => vec![id],
        }
    }

    /// Typing into several panes, or back to one. It starts with all of
    /// the workspace's; a click on a pane leaves it out, or takes it back.
    pub(super) fn toggle_broadcast(&mut self) {
        if let Some(ws) = self.workspaces.get_mut(self.current) {
            ws.broadcast = !ws.broadcast;
            ws.left_out.clear();
        }
        self.refresh_panes();
    }


    /// While typing into several, a pane left out, or taken back in.
    pub(super) fn toggle_multi_pane(&mut self, id: PaneId) {
        let Some(into) = self.typed_into(id) else { return };
        let ws = &mut self.workspaces[self.current];
        if into {
            ws.left_out.push(id);
        } else {
            ws.left_out.retain(|p| *p != id);
        }
        self.refresh_panes();
    }

    /// The pointer over a MULTI tag: lit, and the hand. True when on one.
    fn multi_hover(&mut self, x: i32, y: i32) -> bool {
        let at = self.multi_tags.iter().find(|(r, _, _)| r.contains(x, y)).map(|(_, id, kind)| (*id, *kind));
        if at != self.multi_lit {
            for id in [self.multi_lit.map(|l| l.0), at.map(|a| a.0)].into_iter().flatten() {
                if let Some(p) = self.panes.get_mut(&id) {
                    p.full = true;
                }
            }
            self.multi_lit = at;
            self.redraw();
        }
        if at.is_some() {
            if let Some(w) = &self.window {
                w.set_cursor(CursorIcon::Pointer);
            }
        }
        at.is_some()
    }
    /// While typing into several, whether a pane gets what is typed.
    pub(super) fn typed_into(&self, id: PaneId) -> Option<bool> {
        let ws = self.workspaces.get(self.current)?;
        (ws.broadcast && ws.root.contains(id)).then(|| !ws.left_out.contains(&id))
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

    /// The keys jot keeps for itself, screen's among them. True when
    /// the key was one.
    fn binding(&mut self, code: KeyCode, event_loop: &ActiveEventLoop) -> bool {
        let m = self.mods;
        let (alt, ctrl, shift, sup) = (m.alt_key(), m.control_key(), m.shift_key(), m.super_key());
        // In a popped-out window: back in, close, copy and paste; the
        // rest are the program's.
        if let Some(wid) = self.focused_popout {
            match (code, alt, ctrl, shift) {
                (KeyCode::KeyO, true, false, true) => self.pop_in(wid),
                (KeyCode::KeyW, false, true, false) => {
                    if let Some(id) = self.focused() {
                        self.close(id, event_loop);
                    }
                }
                (KeyCode::KeyC, false, true, true) => self.copy(false),
                (KeyCode::KeyV, false, true, true) => self.paste(false),
                _ => return false,
            }
            return true;
        }
        if alt && !ctrl && !sup {
            match (code, shift) {
                (KeyCode::KeyS, false) => self.split(Dir::Down),
                (KeyCode::KeyV, false) => self.split(Dir::Across),
                (KeyCode::KeyS, true) => self.split_side(Dir::Down, true),
                (KeyCode::KeyV, true) => self.split_side(Dir::Across, true),
                (KeyCode::KeyH, false) => self.focus_dir(-1, 0),
                (KeyCode::KeyJ, false) => self.focus_dir(0, 1),
                (KeyCode::KeyK, false) => self.focus_dir(0, -1),
                (KeyCode::KeyL, false) => self.focus_dir(1, 0),
                (KeyCode::KeyX, false) => {
                    self.close_pane(event_loop);
                }
                (KeyCode::KeyF, true) => {
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
                (KeyCode::KeyM, true) => self.toggle_broadcast(),
                (KeyCode::KeyO, true) => self.pop_out(event_loop),
                (KeyCode::KeyP, true) => self.paste(false),
                (KeyCode::KeyP, false) => self.open_copy_mode(),
                (KeyCode::Backquote, false) => self.toggle_roam(),
                (KeyCode::Semicolon, false) => self.toggle_bar(),
                _ => return false,
            }
            return true;
        }
        if sup && !alt && !ctrl && code == KeyCode::KeyK {
            self.open_palette();
            return true;
        }
        // Ctrl R, as a browser reloads: the running command stopped and run
        // again, Ctrl Shift R killing it outright first; at a prompt, the
        // last one run again. Under a full-screen program (vim's redo) the
        // key is the program's.
        if ctrl && !alt && code == KeyCode::KeyR && self.reload_key(shift) {
            return true;
        }
        if ctrl && shift && !alt {
            match code {
                KeyCode::KeyC => self.copy(false),
                KeyCode::KeyV => self.paste(false),
                KeyCode::KeyT => self.reopen(),
                KeyCode::ArrowUp => self.jump_prompt(-1),
                KeyCode::ArrowDown => self.jump_prompt(1),
                KeyCode::KeyE => self.open_editor(),
                _ => return false,
            }
            return true;
        }
        if ctrl && !shift && !alt && code == KeyCode::KeyK {
            self.open_palette();
            return true;
        }
        if ctrl && !shift && !alt && code == KeyCode::KeyP {
            self.open_palette_with("> ");
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
        // Ctrl N: a new workspace, in this pane's folder, as a browser's new
        // window.
        if ctrl && !shift && !alt && code == KeyCode::KeyN {
            self.new_workspace();
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
        // A pane's command line open: the keys are its, all but its own key
        // and the other bindings, which shut it or act over it.
        if self.bar.is_some() && self.editor.is_none() && event.state == ElementState::Pressed {
            if self.mods.alt_key() && code == KeyCode::Semicolon {
                self.toggle_bar();
                return;
            }
            let word_key = matches!(code, KeyCode::Backspace | KeyCode::Delete | KeyCode::ArrowLeft | KeyCode::ArrowRight | KeyCode::KeyB | KeyCode::KeyF | KeyCode::KeyD);
            if !(self.mods.alt_key() || self.mods.super_key()) || (self.mods.alt_key() && word_key && !self.mods.shift_key()) {
                self.bar_key(&event, event_loop);
                return;
            }
        }
        if self.editor.is_some() {
            if event.state == ElementState::Pressed {
                self.editor_key(&event);
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
        // Slash commands at a prompt see the key first.
        if event.state == ElementState::Pressed && self.slash_key(id, code, text, event_loop) {
            return;
        }
        // Broadcasting, every pane in the workspace gets the key, each as
        // its own program would have it.
        if event.state == ElementState::Pressed {
            self.note_typing(id);
            // Typing is beginning fresh: the offer of the last session goes.
            self.dismiss_offer();
        }
        // Ctrl Backspace at a shell prompt: a word rubbed out, as Alt
        // Backspace (Esc DEL) is to tcsh, bash and zsh. In programs it is
        // left as it is.
        let word_back = code == KeyCode::Backspace && m.control_key() && !m.alt_key() && !m.shift_key() && event.state != ElementState::Released;
        for target in self.input_targets(id) {
            let Some(p) = self.panes.get_mut(&target) else { continue };
            let bytes = if word_back && p.shell.at_prompt { b"\x1b\x7f".to_vec() } else { p.term.encode_key(crate::keys::ghostty(code), mods, action, text, unshifted) };
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
            v.retain(|p| !self.ws().minimized.contains(p) && !self.ws().spaces.contains(p));
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
        if (button == MouseButton::Left && (self.offer_click(event_loop) || self.toast_click() || self.mini_click() || self.pin_click() || self.editor_run_click() || self.editor_press() || self.bar_field_press() || self.bar_click())) || self.settings_click(event_loop) || self.overlay_click(button, event_loop) || (button == MouseButton::Left && self.find_click()) {
            return;
        }
        let (x, y) = self.pointer;
        if button == MouseButton::Left && self.mods.control_key() && self.open_link() {
            return;
        }
        // A pane's corner handle takes it, to drop somewhere else.
        if button == MouseButton::Left {
            if let Some(pane) = self.handle_at(x, y) {
                self.drag = Some(Drag::Pane(dnd::Carry { pane, from: (x, y), moving: false, over: None }));
                if let Some(w) = &self.window {
                    w.set_cursor(CursorIcon::Grabbing);
                }
                return;
            }
        }
        // A tag on a caption: MULTI takes the pane in or out of typing into
        // several; TEE and FILTER stop. Without captions, there being no
        // tags, a click anywhere on a pane takes it in or out.
        if button == MouseButton::Left && !self.roam_shown {
            let tag = self.multi_tags.iter().find(|(r, _, _)| r.contains(x, y)).map(|(_, id, kind)| (*id, *kind));
            let bare = if self.config.captions { None } else { self.pane_at(x, y).map(|(id, _)| id).filter(|id| self.typed_into(*id).is_some()).map(|id| (id, watch::Tag::Multi)) };
            if let Some((id, kind)) = tag.or(bare) {
                match kind {
                    watch::Tag::Multi => self.toggle_multi_pane(id),
                    watch::Tag::Tee => self.open_tag_menu(id, kind),
                    watch::Tag::TeeStop => self.stop_tee(id),
                    watch::Tag::Restart => self.restart(id),
                    watch::Tag::Minimize => self.minimize(id),
                    watch::Tag::Focus => {
                        self.set_focus(id);
                        let ws = self.ws_mut();
                        ws.zoom = if ws.zoom.is_some() { None } else { Some(id) };
                        self.changed();
                    }
                    watch::Tag::Filter => self.stop_filter(id),
                    watch::Tag::Notify => self.set_notify(id, false),
                }
                return;
            }
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
        // The command line's text, right clicked: a text box's menu.
        if button == MouseButton::Right && (self.editor_menu() || self.bar_field_menu() || self.pin_menu() || self.mini_menu() || self.space_menu()) {
            return;
        }
        // A tag on a caption, right clicked: what can be done with it.
        if button == MouseButton::Right {
            if let Some((id, kind)) = self.multi_tags.iter().find(|(r, _, _)| r.contains(x, y)).map(|(_, id, kind)| (*id, *kind)) {
                return self.open_tag_menu(id, kind);
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
                // One click, two (a word), three (the line), then round again.
                // The third may land anywhere on the row the second did.
                let n = match self.last_click {
                    Some((t, c, n)) if t.elapsed() < Duration::from_millis(400) && (c == cell || (n == 2 && c.1 == cell.1)) => n % 3 + 1,
                    _ => 1,
                };
                self.last_click = Some((Instant::now(), cell, n));
                if n > 1 {
                    if n == 2 {
                        p.term.select_word(cell);
                    } else {
                        p.term.select_line(cell);
                    }
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
        if self.offer_hover(x, y) || self.find_hover(x, y) || self.settings_hover(x, y) || self.overlay_hover(x, y) {
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
                if self.mini_hover(x, y) || self.pin_hover(x, y) || self.editor_drag(x, y) || self.editor_run_hover(x, y) || self.editor_hover(x, y) || self.bar_field_drag(x, y) || self.bar_field_hover(x, y) || self.bar_hover(x, y) || self.multi_hover(x, y) || self.handle_hover(x, y) {
                    return;
                }
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
        self.bar_field_release();
        self.editor_release();
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
        self.replay_held();
        while let Ok(req) = self.requests.try_recv() {
            self.answer(req, event_loop);
        }
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
                    let watched = self.panes.get(&id).is_some_and(|p| p.tee.is_some() || p.filter.is_some());
                    let bytes = if watched { self.watch_output(id, bytes) } else { bytes };
                    if let Some(p) = self.panes.get_mut(&id) {
                        p.term.write(&bytes);
                        search_again |= searched == Some(id);
                        if !p.has_images && bytes.windows(3).any(|w| w == b"\x1b_G") {
                            p.has_images = true;
                        }
                        if !shown.contains(&id) && !p.activity && p.started.elapsed() > Duration::from_secs(3) {
                            p.activity = true;
                            self.full = true;
                        }
                        any = true;
                    }
                    self.check_triggers(id, &bytes);
                    if self.is_minimized(id) {
                        self.note_last_line(id, &bytes);
                    }
                    self.shell_events(id);
                    self.queue_tick(id);
                    if let Some(i) = self.popout_of(id) {
                        self.popouts[i].window.request_redraw();
                    }
                }
                Output::Exit(id) => exited.push(id),
                Output::Mode(id, raw) => {
                    if let Some(p) = self.panes.get_mut(&id) {
                        p.pty.raw = raw;
                    }
                    if raw {
                        self.queue_tick(id);
                    }
                }
                Output::Gone => {
                    eprintln!("jot: the session ended");
                    self.mux.detach();
                    event_loop.exit();
                    return;
                }
            }
        }
        if let (true, Some(id)) = (search_again, searched) {
            self.find_output(id);
        }
        for id in exited {
            // Empty space's shell was ended on purpose: the space stays.
            if self.panes.contains_key(&id) && !self.is_space(id) {
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
    /// Each workspace's accent, and whether it was given one rather than
    /// choosing it.
    pub(super) fn ws_accents(&self) -> Vec<(u32, bool)> {
        let named: Vec<(&str, Option<&str>)> = self.workspaces.iter().map(|w| (w.name.as_str(), w.accent.as_deref())).collect();
        crate::theme::workspace_accents(&named, self.accent, self.config.workspace_colors)
    }

    pub(super) fn retint(&mut self) {
        let ws = self.workspaces.get(self.current);
        self.cur_theme = ws.and_then(|w| w.theme.as_deref()).map(Theme::by_name).unwrap_or(self.theme);
        self.cur_accent = self.ws_accents().get(self.current).map_or(self.accent, |a| a.0);
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
        let t0 = Instant::now();
        self.take_resize();
        self.retint();
        self.refresh_captions();
        // With notices up, the frame is drawn whole, so panes do not draw
        // over them.
        let full = std::mem::take(&mut self.full) || !self.toasts.is_empty() || self.offered.is_some() || self.slash.is_some() || self.bar_list_open();
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
        let floating = self.palette.is_some() || self.menu.is_some() || self.settings.is_some() || self.editor.is_some();
        if full || !(self.roam_shown || floating) {
            for (n, id) in ids.iter().enumerate() {
                // Empty space: plain ground, no more.
                if self.ws().spaces.contains(id) {
                    if full {
                        if let Some(r) = self.panes.get(id).map(|p| p.rect) {
                            self.canvas.fill(r, mix(self.cur_theme.bg, 0x000000, 70));
                            self.damage.push(r);
                        }
                    }
                    continue;
                }
                // A minimized pane is its row, drawn apart; its number kept.
                if self.ws().minimized.contains(id) {
                    continue;
                }
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
        if full {
            self.draw_minimized();
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
            self.draw_editor();
            self.draw_toasts();
            self.draw_offer();
            self.draw_slash();
            self.draw_bar_list();
        }
        let drawn = t0.elapsed();
        self.present();
        if crate::debug() {
            eprintln!("frame: drawn {:.2} ms, shown {:.2} ms", drawn.as_secs_f64() * 1000.0, (t0.elapsed() - drawn).as_secs_f64() * 1000.0);
        }
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
        // The cursor moved with nothing written, as a shell moves it back
        // a word with backspaces: its old row and its new are drawn again.
        let cursor = p.term.cursor();
        let cursor_now = cursor.visible.then_some((cursor.x, cursor.y));
        if dirty == 0 && !full && cursor_now == p.cursor_was {
            return;
        }
        let (dfg, dbg) = p.term.colors();
        let rect = p.rect;
        let content = p.content;
        if full {
            self.canvas.fill(Rect { x: rect.x, y: rect.y, w: rect.w, h: rect.h - cap_h }, dbg);
        }
        let mut extra = Vec::new();
        extra.extend(p.cursor_was.map(|(_, y)| y));
        if cursor.visible {
            extra.push(cursor.y);
        }
        p.cursor_was = cursor_now;
        let mut row = Vec::new();
        // With pictures in it, the pane is drawn whole, pictures over text.
        let mut placements = if p.has_images { p.term.placements() } else { Vec::new() };
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
                l.extend(cells.iter().take(cols_n).map(|c| if c.ch == '\0' { ' ' } else { c.ch }));
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
                    let one = |c: &Cell| c.ch.is_ascii_graphic();
                    if !one(&cells[i]) {
                        i += 1;
                        continue;
                    }
                    let bold = cells[i].flags & vt::BOLD != 0;
                    let mut j = i;
                    while j < cells.len() && one(&cells[j]) && (cells[j].flags & vt::BOLD != 0) == bold {
                        j += 1;
                    }
                    // Shaped only when there are symbols in it to join.
                    let symbols = cells[i..j].iter().any(|c| "=<>!-|&*/:.+~#_%^?;\\[]{}()w".contains(c.ch));
                    let run: String = if j - i >= 2 && symbols { cells[i..j].iter().map(|c| c.ch).collect() } else { String::new() };
                    if !run.is_empty() {
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
                    if let Some(chr) = Some(c.ch).filter(|&ch| ch != '\0') {
                        if chr != ' ' {
                            // An emoji is two cells wide unless the next has text in it.
                            let narrow = cells.get(x + 1).is_some_and(|n| n.ch != '\0') || x + 1 == cells.len();
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
        let activity = p.activity;
        if full {
            if self.config.captions {
                let into = self.typed_into(id);
                self.draw_caption(id, rect, n, &caption, focused, activity, into);
            }
        }
        if full {
            self.draw_pins(id);
            self.draw_handle(id);
            self.draw_bar(id);
        }
        self.draw_blocks(id);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_caption(&mut self, id: PaneId, rect: Rect, n: usize, caption: &str, focused: bool, activity: bool, into: Option<bool>) {
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
        // Tags at the right: copy mode, and the pane's watchers and typing
        // into several, the last buttons that toggle or stop.
        let mut right = bar.x + bar.w - pad as i32;
        let copying = self.copy_mode.as_ref().is_some_and(|c| self.panes.get(&c.pane).is_some_and(|p| p.rect == rect));
        self.multi_tags.retain(|(_, p, _)| *p != id);
        // At the far right, the focus button: this pane alone, and back;
        // before it, while a command runs, the restart button.
        // Only where there are panes to hide, or one is alone already.
        let zoomed = self.ws().zoom.is_some();
        let many = {
            let mut ids = Vec::new();
            self.ws().root.panes(&mut ids);
            ids.len() > 1
        };
        if n > 0 && (many || zoomed) && self.ws().root.contains(id) {
            let r = Rect { x: right - h + pad as i32, y: bar.y, w: h, h };
            if self.multi_lit == Some((id, watch::Tag::Focus)) {
                self.canvas.fill(r, mix(bg, fg, 50));
            }
            let size = h * 6 / 10;
            let icon = if zoomed { crate::canvas::icons::UNFOCUS } else { crate::canvas::icons::FOCUS };
            self.canvas.icon(icon, size, r.x + (h - size) / 2, r.y + (h - size) / 2, fg, r);
            self.multi_tags.push((r, id, watch::Tag::Focus));
            right = r.x - pad as i32 / 2;
        }
        // Before it, minimize: the pane to a row at the top, still running.
        if n > 0 && many && !zoomed && self.ws().root.contains(id) {
            let r = Rect { x: right + pad as i32 / 2 - h, y: bar.y, w: h, h };
            if self.multi_lit == Some((id, watch::Tag::Minimize)) {
                self.canvas.fill(r, mix(bg, fg, 50));
            }
            let (lw, lh) = (h * 4 / 10, self.px(2.0).max(2));
            self.canvas.fill(Rect { x: r.x + (h - lw) / 2, y: r.y + h * 6 / 10, w: lw, h: lh }, fg);
            self.multi_tags.push((r, id, watch::Tag::Minimize));
            right = r.x - pad as i32 / 2;
        }
        let running = self.panes.get(&id).is_some_and(|p| p.shell.marked && !p.shell.at_prompt && p.shell.running.is_some());
        if n > 0 && running {
            let r = Rect { x: right + pad as i32 / 2 - h, y: bar.y, w: h, h };
            if self.multi_lit == Some((id, watch::Tag::Restart)) {
                self.canvas.fill(r, mix(bg, fg, 50));
            }
            let size = h * 6 / 10;
            self.canvas.icon(crate::canvas::icons::RESTART, size, r.x + (h - size) / 2, r.y + (h - size) / 2, fg, r);
            self.multi_tags.push((r, id, watch::Tag::Restart));
            right = r.x - pad as i32 / 2;
        }
        // The pane's watchers and its typing into several, each a pill
        // with its icon, that a click stops or toggles.
        let (tee, filter) = self.panes.get(&id).map_or((None, None), |p| (p.tee.as_ref().map(|t| format!("{} ({})", t.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), t.lines)), p.filter.as_ref().map(|f| (f.text.clone(), f.armed))));
        let mut tags: Vec<(watch::Tag, String, u32, &'static [u8])> = Vec::new();
        if let Some(included) = into {
            tags.push((watch::Tag::Multi, if included { "MULTI" } else { "SKIPPED" }.into(), if included { 0xE5484D } else { 0x6F6F6F }, crate::canvas::icons::KEYBOARD));
        }
        if let Some(name) = tee {
            tags.push((watch::Tag::Tee, format!("TEE {name}"), 0x12A594, crate::canvas::icons::TEE));
        }
        if self.panes.get(&id).is_some_and(|p| p.shell.notify) {
            tags.push((watch::Tag::Notify, "NOTIFY".into(), 0xFFC53D, crate::canvas::icons::BELL));
        }
        if let Some((text, armed)) = filter {
            let label = if armed { format!("NEXT: {text}") } else { text };
            tags.push((watch::Tag::Filter, label, 0x8E4EC6, crate::canvas::icons::FILTER));
        }
        for (kind, label, base, icon) in tags {
            let lit = self.multi_lit == Some((id, kind));
            let color = if lit { mix(base, 0xFFFFFF, 40) } else { base };
            let (gap, size) = (self.px(6.0), self.px(15.0).min(h - self.px(4.0)));
            // A tee has a stop button at its end, an × of its own.
            let stop = if kind == watch::Tag::Tee { h } else { 0 };
            let w = (gap + size + gap + self.fonts.ui.width(&label) + gap + stop).min(bar.w / 2);
            let r = Rect { x: right - w, y: bar.y, w, h };
            self.canvas.fill(r, color);
            let fg = crate::canvas::on(color);
            self.canvas.icon(icon, size, r.x + gap, bar.y + (h - size) / 2, fg, r);
            self.canvas.text_fit(&mut self.fonts.ui, &label, r.x + gap + size + gap, ty, r.w - size - gap * 3 - stop, fg, r);
            let body = Rect { w: r.w - stop, ..r };
            if stop > 0 {
                let x = Rect { x: r.x + r.w - stop, y: r.y, w: stop, h };
                let x_lit = self.multi_lit == Some((id, watch::Tag::TeeStop));
                self.canvas.fill(x, if x_lit { mix(base, 0x000000, 60) } else { mix(base, 0x000000, 25) });
                let (cx, cy, arm, dot) = (x.x + stop / 2, x.y + h / 2, self.px(3.5), self.px(1.5).max(2));
                for t in -arm..=arm {
                    self.canvas.round_fill(Rect { x: cx + t - dot / 2, y: cy + t - dot / 2, w: dot, h: dot }, dot / 2, fg, 255);
                    self.canvas.round_fill(Rect { x: cx + t - dot / 2, y: cy - t - dot / 2, w: dot, h: dot }, dot / 2, fg, 255);
                }
                self.multi_tags.push((x, id, watch::Tag::TeeStop));
            }
            self.multi_tags.push((body, id, kind));
            right = r.x - pad as i32 / 2;
        }
        for (on, tag, color) in [(copying, " COPY ", 0xFFC53D)] {
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
        let damage = std::mem::take(&mut self.damage);
        // Mid-resize, the CPU's way, as without a GPU.
        let settling = self.gpu_settle.is_some();
        if let Some(g) = self.gpu.as_mut().filter(|_| !settling) {
            if g.present(&self.canvas, &damage) {
                return;
            }
            // Not drawn this time: all of it goes up the other way.
            let whole = Rect { x: 0, y: 0, w: self.canvas.width, h: self.canvas.height };
            if let (Some(surface), Some(window)) = (self.surface.as_mut(), self.window.as_ref()) {
                present_frame(surface, window, &self.canvas, vec![whole]);
            }
            return;
        }
        if let (Some(surface), Some(window)) = (self.surface.as_mut(), self.window.as_ref()) {
            present_frame(surface, window, &self.canvas, damage);
        }
    }

    /// The workspaces as they are now, written out.
    /// Every pane's scrollback kept, by its place: "1-2" for workspace
    /// 1's second pane, "p1" for the first popped out.
    fn save_scrollback(&mut self) {
        // The last session is left as it was while it is on offer.
        if self.offered.is_some() {
            return;
        }
        let mut dumps = Vec::new();
        for (wi, ws) in self.workspaces.iter().enumerate() {
            let mut ids = Vec::new();
            ws.root.panes(&mut ids);
            for (pi, id) in ids.iter().enumerate() {
                if let Some(text) = self.panes.get(id).and_then(|p| p.term.dump(5000, p.shell.at_prompt.then_some(p.shell.prompt_row))) {
                    dumps.push((format!("{}-{}", wi + 1, pi + 1), text));
                }
            }
        }
        for (i, po) in self.popouts.iter().enumerate() {
            if let Some(text) = self.panes.get(&po.pane).and_then(|p| p.term.dump(5000, p.shell.at_prompt.then_some(p.shell.prompt_row))) {
                dumps.push((format!("p{}", i + 1), text));
            }
        }
        persist::save_scrollback(&dumps);
        self.scrollback_at = Instant::now();
    }

    fn save(&mut self) {
        if self.offered.is_some() {
            return;
        }
        let panes = &self.panes;
        let spaces: Vec<PaneId> = self.workspaces.iter().flat_map(|w| w.spaces.iter().copied()).collect();
        let describe = |id: PaneId| match panes.get(&id) {
            Some(p) => (p.cmd.clone(), p.pty.cwd(), p.pins.iter().map(|pin| pin.cmd.clone()).collect(), spaces.contains(&id)),
            None => (String::new(), None, Vec::new(), spaces.contains(&id)),
        };
        let workspaces = self
            .workspaces
            .iter()
            .map(|ws| WorkspaceSpec { name: ws.name.clone(), layout: Some(persist::spec(&ws.root, &describe)), accent: ws.accent.clone(), theme: ws.theme.clone(), on_open: ws.on_open.clone(), on_focus: ws.on_focus.clone(), ..Default::default() })
            .collect();
        let roam = self.roam.and_then(|r| panes.get(&r)).and_then(|p| p.pty.cwd()).or(self.roam_cwd.clone()).map(|p| persist::shorten(&p));
        let popouts = self.popouts.iter().map(|po| persist::spec(&Node::Pane(po.pane), &describe)).collect();
        let session = Session { current: self.current, roam, workspaces, popouts };
        // The session keeps it too, for the next window to take up.
        if let Ok(text) = toml::to_string(&session) {
            self.mux.layout(text);
        }
        persist::save_session(&session);
        self.saved_at = Instant::now();
        self.unsaved = false;
    }
}

impl ApplicationHandler<()> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = window_attributes("jot", LogicalSize::new(1400.0, 900.0));
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
        // The GPU, unless the settings say the CPU; the CPU too when there
        // is no GPU to use.
        if self.config.renderer != "cpu" {
            // The window is kept in self.window, and the GPU, declared
            // before it, goes first.
            self.gpu = unsafe { crate::gpu::Gpu::new(&window, event_loop.owned_display_handle(), size.width, size.height) };
        }
        if crate::debug() {
            eprintln!("drawn with {}", self.gpu.as_ref().map_or("the CPU".to_string(), |g| format!("the GPU: {}", g.name)));
        }
        self.window = Some(window);
        self.start();
        self.open_pending_popouts(event_loop);
        self.load_recent();
        self.set_title();
        self.relayout();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _: ()) {
        self.drain(event_loop);
        self.collect_ran();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, wid: WindowId, event: WindowEvent) {
        // A popped-out pane's window: its own drawing, mouse and size;
        // its keys go where every key goes, to the pane with the keyboard.
        if self.window.as_ref().is_some_and(|w| w.id() != wid) {
            let keys = matches!(event, WindowEvent::KeyboardInput { .. } | WindowEvent::ModifiersChanged(_));
            if keys || !self.popout_event(wid, &event) {
                if !keys {
                    return;
                }
            } else {
                return;
            }
        }
        match event {

            WindowEvent::CloseRequested => {
                self.save_scrollback();
                self.save();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                // Taken up when the frame is drawn, the last size only.
                self.resized = Some((size.width, size.height));
                self.redraw();
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
                    // Three lines a notch, more the faster the wheel turns: a
                    // flick goes a long way, a slow turn line by line.
                    MouseScrollDelta::LineDelta(_, y) => {
                        let quick = self.wheel_at.elapsed() < Duration::from_millis(60);
                        self.wheel_streak = if quick { (self.wheel_streak + 1).min(8) } else { 0 };
                        self.wheel_at = Instant::now();
                        -(y * 3.0 * (1.0 + self.wheel_streak as f32 / 2.0)) as isize
                    }
                    MouseScrollDelta::PixelDelta(p) => -(p.y / self.fonts.cell_h as f64) as isize,
                };
                if lines != 0 && !self.settings_scroll(lines) && !self.pins_wheel(lines as f32) {
                    self.scroll(lines);
                }
            }
            WindowEvent::Focused(on) => {
                self.focused_window = on;
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
        self.expire_toasts();
        if ENDING.load(Ordering::SeqCst) {
            self.save_scrollback();
            self.save();
            return event_loop.exit();
        }
        // The panes' scrollback is kept every five minutes as well as at the end.
        if self.scrollback_at.elapsed() > Duration::from_secs(300) {
            self.save_scrollback();
        }
        if self.saved_at.elapsed() > Duration::from_secs(30) {
            self.save();
        }
        // A queued command waiting for the shell's line editor: looked at
        // again soon.
        let due: Vec<PaneId> = self.panes.iter().filter(|(_, p)| p.shell.queue_due).map(|(id, _)| *id).collect();
        for id in &due {
            self.queue_tick(*id);
        }
        let printing = self.print_tick();
        let soon = if due.is_empty() && !printing { Duration::from_secs(1) } else { Duration::from_millis(50) };
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + soon));
        // The window has kept its size: back to the GPU, set up for it
        // when the frame, all of it, is sent up.
        if let Some(at) = self.gpu_settle {
            if at <= Instant::now() {
                self.gpu_settle = None;
                self.full = true;
                self.redraw();
            } else {
                event_loop.set_control_flow(ControlFlow::WaitUntil(at.min(Instant::now() + soon)));
            }
        }
        if self.captions_at.elapsed() >= Duration::from_secs(1) {
            self.redraw();
        }
    }
}

/// What was drawn on a canvas, put on its window: the parts that changed,
/// or all of it when the window's buffer is not the last one shown.
pub(super) fn present_frame(surface: &mut Surface<Rc<Window>, Rc<Window>>, window: &Window, canvas: &Canvas, damage: Vec<Rect>) {
    if damage.is_empty() {
        return;
    }
    let Ok(mut buffer) = surface.buffer_mut() else { return };
    let width = canvas.width as usize;
    if buffer.len() != canvas.pixels.len() {
        return;
    }
    let whole = buffer.age() != 1 || damage.iter().any(|d| d.w == canvas.width && d.h == canvas.height);
    let rects: Vec<softbuffer::Rect> = if whole {
        buffer.copy_from_slice(&canvas.pixels);
        vec![]
    } else {
        damage
            .iter()
            .filter_map(|d| {
                let x0 = d.x.clamp(0, canvas.width) as usize;
                let x1 = (d.x + d.w).clamp(0, canvas.width) as usize;
                let y0 = d.y.clamp(0, canvas.height) as usize;
                let y1 = (d.y + d.h).clamp(0, canvas.height) as usize;
                if x0 >= x1 || y0 >= y1 {
                    return None;
                }
                for y in y0..y1 {
                    buffer[y * width + x0..y * width + x1].copy_from_slice(&canvas.pixels[y * width + x0..y * width + x1]);
                }
                Some(softbuffer::Rect { x: x0 as u32, y: y0 as u32, width: NonZeroU32::new((x1 - x0) as u32)?, height: NonZeroU32::new((y1 - y0) as u32)? })
            })
            .collect()
    };
    window.pre_present_notify();
    let _ = if whole || rects.is_empty() { buffer.present() } else { buffer.present_with_damage(&rects) };
}

/// What a pane held when jot last ended, shown above its new prompt,
/// with a line saying when it was kept.
fn restore_scrollback(pane: Option<&mut Pane>, name: &str) {
    let (Some(p), Some((text, when))) = (pane, persist::load_scrollback(name)) else { return };
    let ago = when.elapsed().map(commands::human).unwrap_or_default();
    let line = format!("\x1b[0m\r\n\x1b[2m── kept from the last run, {ago} ago ──\x1b[0m\r\n");
    p.term.write(text.as_bytes());
    p.term.write(line.as_bytes());
}

/// Set when jot is asked to end (SIGTERM, SIGHUP, as at a logout): the
/// next turn of the loop keeps everything and ends.
static ENDING: AtomicBool = AtomicBool::new(false);

extern "C" fn on_end_signal(_: libc::c_int) {
    ENDING.store(true, Ordering::SeqCst);
}

pub fn catch_end_signals() {
    unsafe {
        libc::signal(libc::SIGTERM, on_end_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
        libc::signal(libc::SIGHUP, on_end_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
    }
}

impl App {
    /// The panes that were in windows of their own, in them again.
    pub(super) fn open_pending_popouts(&mut self, event_loop: &ActiveEventLoop) {
        for (i, spec) in std::mem::take(&mut self.pending_popouts).into_iter().enumerate() {
            let (cmd, cwd, pane) = match spec {
                persist::Spec::Pane { cmd, cwd, id, .. } => (cmd, cwd.map(|c| persist::expand(&c)).unwrap_or_else(pty::home), id),
                persist::Spec::Command(cmd) => (cmd, pty::home(), None),
                _ => continue,
            };
            let held = pane.is_some_and(|p| self.held.contains_key(&p));
            let id = self.spawn(Start { cmd, cwd, pane });
            if !held {
                restore_scrollback(self.panes.get_mut(&id), &format!("p{}", i + 1));
            }
            self.open_popout_window(id, event_loop);
        }
        self.replay_held();
    }
}

/// jot's own window settings: its icon, and its name for the desktop
/// (the X11 class and the Wayland app id), so that a dock or a list of
/// windows shows it as jot, with jot.desktop's icon.
pub(super) fn window_attributes(title: &str, size: impl Into<winit::dpi::Size>) -> winit::window::WindowAttributes {
    use winit::platform::wayland::WindowAttributesExtWayland;
    use winit::platform::x11::WindowAttributesExtX11;
    let attrs = Window::default_attributes().with_title(title).with_inner_size(size).with_window_icon(app_icon());
    let attrs = WindowAttributesExtX11::with_name(attrs, "jot", "jot");
    WindowAttributesExtWayland::with_name(attrs, "jot", "jot")
}

/// The icon built in: a prompt on tomato, at 64 pixels, as a window's is
/// shown; X11 takes no more in one request than a 256 one would need.
fn app_icon() -> Option<winit::window::Icon> {
    static PNG: &[u8] = include_bytes!("../assets/jot.png");
    let mut reader = png::Decoder::new(std::io::Cursor::new(PNG)).read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    let (w, h) = (info.width as usize, info.height as usize);
    let k = (w / 64).max(1);
    let (ow, oh) = (w / k, h / k);
    let mut out = vec![0u8; ow * oh * 4];
    for y in 0..oh {
        for x in 0..ow {
            // Each pixel the mean of the block it stands for, its colors
            // weighed by how opaque they are, so edges do not darken.
            let mut sum = [0u32; 4];
            for dy in 0..k {
                for dx in 0..k {
                    let i = ((y * k + dy) * w + x * k + dx) * 4;
                    let a = buf[i + 3] as u32;
                    for c in 0..3 {
                        sum[c] += buf[i + c] as u32 * a;
                    }
                    sum[3] += a;
                }
            }
            let o = (y * ow + x) * 4;
            for c in 0..3 {
                out[o + c] = if sum[3] == 0 { 0 } else { (sum[c] / sum[3]) as u8 };
            }
            out[o + 3] = (sum[3] / (k * k) as u32) as u8;
        }
    }
    winit::window::Icon::from_rgba(out, ow as u32, oh as u32).ok()
}

#[cfg(test)]
mod icon_test {
    #[test]
    fn icon_decodes() {
        assert!(super::app_icon().is_some());
    }
}

impl Drop for App {
    /// The window going: its panes left running in the session, for the
    /// next window to take up. (A pane closed while the window is open
    /// still ends.)
    fn drop(&mut self) {
        self.mux.detach();
    }
}
