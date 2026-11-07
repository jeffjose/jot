//! jterm: a small, fast terminal for Linux, on X11 and Wayland, with
//! splits and workspaces and screen's keys.

mod app;
mod canvas;
mod field;
mod emoji;
mod font;
mod gpu;
mod keys;
mod layout;
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
jterm: a terminal with splits and workspaces

  jterm            start, offering to restore the last session
  jterm --fresh    start from the workspaces in the config
  jterm --debug    print each key as it is sent to the program
  jterm list | open DIR | split [--down] [-- CMD] | run CMD
        | send PANE TEXT | focus PANE | notify TEXT
                   control the running jterm
  jterm --version

Keys:
  Alt v / Alt s        split right / split down
  Alt h j k l          move to the pane left, down, up or right
  Ctrl t               new pane to the right
  Ctrl w, Alt x        close pane
  Ctrl Shift t         reopen closed pane
  Alt Shift f          focus pane / show all
  Alt Shift m          type into multiple panes
  Alt Shift o          move pane to a new window, and back
  Alt arrows, Alt =    resize pane, equalize pane sizes
  Alt 1-9, Alt , .     go to workspace, previous / next workspace
  Alt Shift n          new workspace, in this pane's folder
  Alt `                toggle roaming terminal
  Alt p                copy mode
  Ctrl f               find in pane (Enter: previous match, Shift Enter: next)
  Ctrl k               go to workspace, pane or folder
  Ctrl p               command palette
  Ctrl ,               settings
  Ctrl Shift c / v     copy / paste (selecting copies; middle click pastes)
  Shift PageUp/Down    scroll up / down a page
  Ctrl = - 0           zoom in, out, reset

Settings and workspaces: ~/.config/jterm/config.toml";

pub fn version() -> String {
    let sha = option_env!("VERGEN_GIT_SHA").filter(|s| !s.is_empty() && !s.starts_with("VERGEN"));
    match sha {
        Some(sha) => format!("jterm {} ({sha})", env!("CARGO_PKG_VERSION")),
        None => format!("jterm {}", env!("CARGO_PKG_VERSION")),
    }
}

fn main() {
    // The `jterm` command: a request to the jterm running.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| remote::COMMANDS.contains(&a.as_str())) {
        std::process::exit(remote::client(&args));
    }
    let mut fresh = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--fresh" => fresh = true,
            "--debug" => DEBUG.store(true, std::sync::atomic::Ordering::Relaxed),
            "-V" | "--version" => return println!("{}", version()),
            "-h" | "--help" => return println!("{HELP}"),
            other => {
                eprintln!("jterm: unknown option {other}\n\n{HELP}");
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
