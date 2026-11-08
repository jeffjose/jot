//! The `jot` command, to drive the jot running from any shell or
//! script: a socket in the runtime folder, a request a line of fields
//! split by tabs, the answer text until the socket closes.
//!
//!   jot list                     the workspaces and their panes
//!   jot open DIR                 a workspace in DIR, or the one there
//!   jot split [--down] [-- CMD]  a pane beside the one you are in
//!   jot run CMD                  a pane to the right running CMD
//!   jot send PANE TEXT           TEXT typed into PANE (1.2: workspace 1,
//!                                  pane 2); \n is Enter
//!   jot focus PANE               go to PANE
//!   jot notify TEXT              a notice on the desktop

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

pub const COMMANDS: &[&str] = &["list", "open", "split", "run", "send", "focus", "notify"];

pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir());
    dir.join(format!("jot-{}.sock", unsafe { libc::getuid() }))
}

/// A request from the command, and where its answer goes.
pub struct Request {
    pub args: Vec<String>,
    pub reply: Sender<String>,
}

/// The quake window's own socket, which `jot --quake` toggles it by.
pub fn quake_socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir());
    dir.join(format!("jot-{}-quake.sock", unsafe { libc::getuid() }))
}

/// Listens for the command on a thread of its own, handing each request
/// over with `send`, which also wakes the window.
pub fn listen(send: impl Fn(Request) + Send + 'static) {
    listen_at(socket_path(), send);
}

/// As listen, on the socket at `path`.
pub fn listen_at(path: PathBuf, send: impl Fn(Request) + Send + 'static) {
    // One left by a jot that ended is cleared; one that answers is kept.
    if UnixStream::connect(&path).is_ok() {
        return;
    }
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => return eprintln!("jot: the jot command will not reach this jot: {}: {e}", path.display()),
    };
    std::thread::Builder::new()
        .name("remote".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                let mut reader = BufReader::new(&stream);
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let args: Vec<String> = line.trim_end_matches('\n').split('\t').map(|a| a.replace("\\t", "\t").replace("\\n", "\n")).collect();
                let (tx, rx) = std::sync::mpsc::channel();
                send(Request { args, reply: tx });
                let answer = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap_or_else(|_| "jot did not answer\n".into());
                let _ = (&stream).write_all(answer.as_bytes());
            }
        })
        .expect("remote thread");
}

/// The command side: sends the request and prints the answer.
pub fn client(args: &[String]) -> i32 {
    let Ok(mut stream) = UnixStream::connect(socket_path()) else {
        eprintln!("jot: no jot is running to {} in", args.first().map(String::as_str).unwrap_or("send to"));
        return 1;
    };
    let line: Vec<String> = args.iter().map(|a| a.replace('\t', "\\t").replace('\n', "\\n")).collect();
    if stream.write_all(format!("{}\n", line.join("\t")).as_bytes()).is_err() {
        return 1;
    }
    let mut answer = String::new();
    let _ = stream.read_to_string(&mut answer);
    print!("{answer}");
    if answer.starts_with("jot:") { 1 } else { 0 }
}

/// `jot --quake`: the quake window shown, or hidden if it shows; started
/// if there is none.
pub fn quake() -> i32 {
    if let Ok(mut stream) = UnixStream::connect(quake_socket_path()) {
        if stream.write_all(b"toggle\n").is_ok() {
            let mut answer = String::new();
            let _ = stream.read_to_string(&mut answer);
            return 0;
        }
    }
    // None running: one started, apart from this command, in its own
    // session so that it outlives the shell that ran this.
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("jot"));
    let quiet = || std::process::Stdio::null();
    let started = unsafe {
        std::process::Command::new(exe)
            .arg("--quake-window")
            .stdin(quiet())
            .stdout(quiet())
            .stderr(quiet())
            .pre_exec(|| {
                libc::setsid();
                Ok(())
            })
            .spawn()
    };
    match started {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("jot: could not start the quake window: {e}");
            1
        }
    }
}

/// `jot restart`, to the quake window if one runs: started again as `exe`
/// (see the window's "restart"). Whether it said it would.
pub fn quake_restart(exe: &str) -> bool {
    let Ok(mut stream) = UnixStream::connect(quake_socket_path()) else { return false };
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    if stream.write_all(format!("restart\t{}\n", exe.replace('\t', "\\t").replace('\n', "\\n")).as_bytes()).is_err() {
        return false;
    }
    let mut answer = String::new();
    let _ = stream.read_to_string(&mut answer);
    answer.starts_with("restarting")
}
