//! jot: a small, fast terminal for Linux, on X11 and Wayland, with
//! splits and workspaces and screen's keys.

mod app;
mod canvas;
mod field;
mod emoji;
mod font;
mod gpu;
mod keys;
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

/// Whether --debug was given: each key is printed as it is sent.
pub fn debug() -> bool {
    DEBUG.load(std::sync::atomic::Ordering::Relaxed)
}

const HELP: &str = "\
jot: a terminal with splits and workspaces

  jot            start, offering to restore the last session
  jot --fresh    start from the workspaces in the config
  jot --debug    print each key as it is sent to the program
  jot list | open DIR | split [--down] [-- CMD] | run CMD
        | send PANE TEXT | focus PANE | notify TEXT
                   control the running jot
  jot setup      add jot and its icon to the desktop's programs
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
  Alt 1-9, Alt , .     go to workspace, previous / next workspace
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
    if args.first().is_some_and(|a| a == "setup") {
        return setup();
    }
    // `jot serve PATH`: a session, started by a window.
    if args.first().is_some_and(|a| a == "serve") {
        let Some(path) = args.get(1) else { return eprintln!("jot serve: which socket?") };
        return mux::server::run(std::path::Path::new(path));
    }
    let mut fresh = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--fresh" => fresh = true,
            "--debug" => DEBUG.store(true, std::sync::atomic::Ordering::Relaxed),
            "-V" | "--version" => return println!("{}", version()),
            "-h" | "--help" => return println!("{HELP}"),
            other => {
                eprintln!("jot: unknown option {other}\n\n{HELP}");
                std::process::exit(2);
            }
        }
    }
    let config = persist::load_config();
    let session = if fresh { None } else { persist::load_session() };
    vt::set_up();
    app::catch_end_signals();
    let event_loop = EventLoop::<()>::with_user_event().build().expect("no display to open a window on");
    let mut app = app::App::new(event_loop.create_proxy(), config, session);
    event_loop.run_app(&mut app).expect("event loop");
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
