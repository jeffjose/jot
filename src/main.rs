//! jot: a small, fast terminal for Linux, on X11 and Wayland, with
//! splits and workspaces and screen's keys.

mod app;
mod canvas;
mod field;
mod emoji;
mod font;
mod gpu;
mod keys;
mod debuglog;
mod layout;
mod mux;
mod persist;
mod palette;
mod pty;
mod remote;
mod theme;
mod vt;

use winit::event_loop::EventLoop;

static DEBUG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether --debug was given, JOT_DEBUG=1, or the switch is on (`jot debug
/// on`, or Settings): each key printed as it is sent, and what jot does
/// kept in debug.log (see debuglog).
pub fn debug() -> bool {
    static FROM_ENV: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FROM_ENV.get_or_init(|| std::env::var_os("JOT_DEBUG").is_some_and(|v| !v.is_empty() && v != "0")) || DEBUG.load(std::sync::atomic::Ordering::Relaxed) || debuglog::switched_on()
}

static RESTORED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static RESTORING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Opened with a window of the last session to bring back (--restore):
/// brought back at once, not offered.
pub fn restoring() -> bool {
    RESTORING.load(std::sync::atomic::Ordering::Relaxed)
}

/// Opened by another window, for the workspaces of a window that quit:
/// it opens no more of them.
pub fn restored() -> bool {
    RESTORED.load(std::sync::atomic::Ordering::Relaxed)
}

static QUAKE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether this is the quake window (`jot --quake` started it).
pub fn quake_mode() -> bool {
    QUAKE.load(std::sync::atomic::Ordering::Relaxed)
}

const HELP: &str = "\
jot: a terminal with splits and workspaces

  jot            start, offering to restore the last session
  jot --fresh    start from the workspaces in the config
  jot --debug    print each key as it is sent to the program
  jot debug on | off | toggle | status
                 every jot here writes what it does to
                 ~/.local/state/jot/debug.log, from now, no restart
  jot list | open DIR | split [--down] [-- CMD] | run CMD
        | send PANE TEXT | focus PANE | notify TEXT
                   control the running jot
  jot --quake    show or hide the quake (drop-down) terminal; bind a key to it
  jot setup      add jot and its icon to the desktop's programs
  jot ssh HOST    a window on HOST's session (jot installed there)
  jot status      what the session holds, and whether a window shows it
  jot restart     swap the session's server for this jot (after an update), every pane kept running
  jot kill        end the session and every pane in it
  jot serve SOCK  a session (started by jot itself)
  jot --version

Keys:
  Alt v / Alt s        split right / split down
  Alt h j k l          move to the pane left, down, up or right
  Ctrl t               new pane to the right
  Ctrl w, Alt x        close pane
  Ctrl Shift t         reopen closed pane
  Alt Shift f          focus pane / show all
  Alt Shift m          multi: type into every pane
  Alt ;                pane command line: run, or queue behind what runs
  Ctrl r               restart the running command (Ctrl Shift r: kill it first)
  Alt Shift o          move pane to a new window, and back
  Alt arrows, Alt =    resize pane, equalize pane sizes
  Alt 1-9              go to workspace
  Alt , / Alt .        the terminal before / after in this pane (those no pane shows)
  Alt '                show another terminal in this pane
  Ctrl n, Alt Shift n  new workspace, in this pane's folder
  Alt `                toggle roaming terminal
  Alt p                copy mode
  Ctrl f               find in pane (Enter: previous match, Shift Enter: next)
  Ctrl k               go to workspace, pane or folder
  Ctrl p               command palette
  Ctrl ,               settings
  Ctrl Shift c / v     copy / paste (selecting copies; middle click pastes)
  Shift PageUp/Down    scroll up / down a page
  Ctrl = - 0           zoom in, out, reset

Closing the window leaves its panes running; starting jot again takes them up,
screens and scrollback as they were. Ctrl P, Quit and end all panes, ends them.

Settings and workspaces: ~/.config/jot/config.toml";

pub fn version() -> String {
    let sha = option_env!("VERGEN_GIT_SHA").filter(|s| !s.is_empty() && !s.starts_with("VERGEN"));
    match sha {
        Some(sha) => format!("jot {} ({sha})", env!("CARGO_PKG_VERSION")),
        None => format!("jot {}", env!("CARGO_PKG_VERSION")),
    }
}

fn main() {
    // The `jot` command: a request to the jot running.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| remote::COMMANDS.contains(&a.as_str())) {
        std::process::exit(remote::client(&args));
    }
    if args.first().is_some_and(|a| a == "debug") {
        std::process::exit(debuglog::command(&args));
    }
    if args.first().is_some_and(|a| a == "setup") {
        return setup();
    }
    // `jot --quake`: the quake window, shown or hidden (started if none is).
    if args.first().is_some_and(|a| a == "--quake") {
        std::process::exit(remote::quake());
    }
    // `jot serve PATH`: a session, started by a window.
    if args.first().is_some_and(|a| a == "serve") {
        let Some(path) = args.get(1) else { return eprintln!("jot serve: which socket?") };
        let handover = args.iter().position(|a| a == "--handover").and_then(|i| args.get(i + 1)).map(std::path::Path::new);
        return mux::server::run(std::path::Path::new(path), handover);
    }
    // The session: what it holds, or ended.
    if args.first().is_some_and(|a| a == "status" || a == "kill" || a == "restart") {
        std::process::exit(mux::command(&args));
    }
    // `jot bridge`: run by ssh, from a window on another machine.
    if args.first().is_some_and(|a| a == "bridge") {
        std::process::exit(mux::bridge());
    }
    // `jot ssh HOST`: a window on HOST's session.
    let host = match args.first().map(String::as_str) {
        Some("ssh") => match args.get(1) {
            Some(h) => Some(h.clone()),
            None => {
                eprintln!("jot ssh: which machine?");
                std::process::exit(2);
            }
        },
        _ => None,
    };
    let mut fresh = false;
    // `--restore FILE`: a window of the last session, opened by the window
    // that restored it, its workspaces in FILE (taken away once read).
    let mut restore_from: Option<std::path::PathBuf> = None;
    let mut args = std::env::args().skip(if host.is_some() { 3 } else { 1 });
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--fresh" => fresh = true,
            "--quake-window" => QUAKE.store(true, std::sync::atomic::Ordering::Relaxed),
            "--debug" => DEBUG.store(true, std::sync::atomic::Ordering::Relaxed),
            "--restored" => RESTORED.store(true, std::sync::atomic::Ordering::Relaxed),
            "--restore" => {
                RESTORED.store(true, std::sync::atomic::Ordering::Relaxed);
                restore_from = args.next().map(std::path::PathBuf::from);
            }
            "-V" | "--version" => return println!("{}", version()),
            "-h" | "--help" => return println!("{HELP}"),
            other => {
                eprintln!("jot: unknown option {other}\n\n{HELP}");
                std::process::exit(2);
            }
        }
    }
    let config = persist::load_config();
    // The quake window has its own panes; the last session saved is the main
    // window's.
    // A window opened by another has the workspaces it is given, if any:
    // the last session is not offered again.
    let session = match &restore_from {
        Some(path) => {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            let _ = std::fs::remove_file(path);
            toml::from_str::<persist::Session>(&text).ok()
        }
        None if fresh || quake_mode() || restored() => None,
        None => persist::load_session(),
    };
    RESTORING.store(restore_from.is_some(), std::sync::atomic::Ordering::Relaxed);
    vt::set_up();
    app::catch_end_signals();
    let event_loop = EventLoop::<()>::with_user_event().build().expect("no display to open a window on");
    let mut app = app::App::new(event_loop.create_proxy(), config, if host.is_some() { None } else { session }, host);
    event_loop.run_app(&mut app).expect("event loop");
    // The window closed, what it keeps saved: the process ends now, nothing
    // torn down first. Freeing the GPU, the window and every pane's terminal
    // took a beat or two, for nothing: the panes run on in the session,
    // which takes the closed connection as the window gone.
    crate::debuglog::log("quit: event loop done");
    std::process::exit(0);
}

/// `jot setup`: jot in the desktop's list of programs, with its icon,
/// for the user who runs it.
fn setup() {
    let data = std::env::var_os("XDG_DATA_HOME").map(std::path::PathBuf::from).unwrap_or_else(|| pty::home().join(".local/share"));
    let exe = std::env::current_exe().map(|e| e.display().to_string()).unwrap_or_else(|_| "jot".into());
    let desktop = include_str!("../assets/jot.desktop").replace("Exec=jot", &format!("Exec={exe}"));
    let files: [(std::path::PathBuf, &[u8]); 3] = [
        (data.join("applications/jot.desktop"), desktop.as_bytes()),
        (data.join("icons/hicolor/scalable/apps/jot.svg"), include_bytes!("../assets/jot.svg")),
        (data.join("icons/hicolor/256x256/apps/jot.png"), include_bytes!("../assets/jot.png")),
    ];
    for (path, bytes) in files {
        let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&path, bytes));
        match written {
            Ok(()) => println!("wrote {}", path.display()),
            Err(e) => {
                eprintln!("jot: could not write {}: {e}", path.display());
                std::process::exit(1);
            }
        }
    }
    // The desktop's caches, told, where it has the tools.
    let quiet = || std::process::Stdio::null();
    let _ = std::process::Command::new("gtk-update-icon-cache").arg("-q").arg(data.join("icons/hicolor")).stdout(quiet()).stderr(quiet()).status();
    let _ = std::process::Command::new("update-desktop-database").arg("-q").arg(data.join("applications")).stdout(quiet()).stderr(quiet()).status();
}
