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

  jterm            bring back the workspaces as you left them
  jterm --fresh    start from the workspaces in the config instead
  jterm --debug    print each key as it is sent to the program
  jterm list | open DIR | split [--down] [-- CMD] | run CMD
        | send PANE TEXT | focus PANE | notify TEXT
                   drive the jterm that is running
  jterm --version

Keys (screen's, on Alt):
  Alt s / Alt v        split: one above another / side by side
  Alt h j k l          move to the pane left, down, up, right
  Ctrl t               a new pane to the right of this one
  Ctrl w, Alt x        close the pane
  Ctrl Shift t         bring back the pane closed last
  Alt Shift m          the pane alone, and back
  Alt arrows, Alt =    resize, make all equal
  Alt 1-9, Alt , .     go to a workspace, the one before or after
  Alt Shift n          a new workspace, in this pane's folder
  Alt `                the roaming terminal, shown over any workspace
  Ctrl f               find in the pane: Enter for the match above, Shift Enter below
  Ctrl k               find a workspace, pane or project folder
  Ctrl p               run a command
  Ctrl ,               settings: theme, text size, captions
  Ctrl Shift c / v     copy / paste (a selection is copied as you make it;
                       middle click pastes it)
  Shift PageUp/Down    scroll back
  Ctrl = - 0           text larger, smaller, as it was

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
