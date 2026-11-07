//! The `jterm` command, to drive the jterm running from any shell or
//! script: a socket in the runtime folder, a request a line of fields
//! split by tabs, the answer text until the socket closes.
//!
//!   jterm list                     the workspaces and their panes
//!   jterm open DIR                 a workspace in DIR, or the one there
//!   jterm split [--down] [-- CMD]  a pane beside the one you are in
//!   jterm run CMD                  a pane to the right running CMD
//!   jterm send PANE TEXT           TEXT typed into PANE (1.2: workspace 1,
//!                                  pane 2); \n is Enter
//!   jterm focus PANE               go to PANE
//!   jterm notify TEXT              a notice on the desktop

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

pub const COMMANDS: &[&str] = &["list", "open", "split", "run", "send", "focus", "notify"];

pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir());
    dir.join(format!("jterm-{}.sock", unsafe { libc::getuid() }))
}

/// A request from the command, and where its answer goes.
pub struct Request {
    pub args: Vec<String>,
    pub reply: Sender<String>,
}

/// Listens for the command on a thread of its own, handing each request
/// over with `send`, which also wakes the window.
pub fn listen(send: impl Fn(Request) + Send + 'static) {
    let path = socket_path();
    // One left by a jterm that ended is cleared; one that answers is kept.
    if UnixStream::connect(&path).is_ok() {
        return;
    }
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => return eprintln!("jterm: the jterm command will not reach this jterm: {}: {e}", path.display()),
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
                let answer = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap_or_else(|_| "jterm did not answer\n".into());
                let _ = (&stream).write_all(answer.as_bytes());
            }
        })
        .expect("remote thread");
}

/// The command side: sends the request and prints the answer.
pub fn client(args: &[String]) -> i32 {
    let Ok(mut stream) = UnixStream::connect(socket_path()) else {
        eprintln!("jterm: no jterm is running to {} in", args.first().map(String::as_str).unwrap_or("send to"));
        return 1;
    };
    let line: Vec<String> = args.iter().map(|a| a.replace('\t', "\\t").replace('\n', "\\n")).collect();
    if stream.write_all(format!("{}\n", line.join("\t")).as_bytes()).is_err() {
        return 1;
    }
    let mut answer = String::new();
    let _ = stream.read_to_string(&mut answer);
    print!("{answer}");
    if answer.starts_with("jterm:") { 1 } else { 0 }
}
