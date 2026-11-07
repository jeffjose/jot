//! `jot serve`: a session. It starts each pane's shell on a PTY of its
//! own, keeps what it prints, and passes keys and output between the
//! shells and the one window attached. It runs on when the window goes,
//! and ends with its last pane.

use super::*;
use crate::pty::Local;
use std::collections::{HashMap, VecDeque};
use std::os::unix::net::UnixListener;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::time::Instant;

/// How much of a pane's output is kept, before the oldest half goes.
const CAP: usize = 8 << 20;

/// Starts a session's server for `path`, apart from this process: its
/// own session (setsid), so that the window's ending does not end it.
pub fn launch(path: &Path) -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let log = crate::persist::state_dir().join("serve.log");
    let _ = std::fs::create_dir_all(crate::persist::state_dir());
    let err = std::fs::OpenOptions::new().create(true).append(true).open(log).map(std::process::Stdio::from).unwrap_or_else(|_| std::process::Stdio::null());
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("serve").arg(path).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(err);
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn()?;
    Ok(())
}

/// What a pane printed, as it came, and the sizes it was made.
#[derive(Default)]
struct Log {
    entries: VecDeque<Kept>,
    bytes: usize,
    /// The size before the first entry kept.
    base: (u16, u16),
}

enum Kept {
    Bytes(Vec<u8>),
    Size(u16, u16),
}

impl Log {
    fn push(&mut self, b: &[u8]) {
        self.bytes += b.len();
        match self.entries.back_mut() {
            Some(Kept::Bytes(last)) if last.len() < 64 << 10 => last.extend_from_slice(b),
            _ => self.entries.push_back(Kept::Bytes(b.to_vec())),
        }
        if self.bytes > CAP {
            self.trim();
        }
    }

    fn size(&mut self, cols: u16, rows: u16) {
        if self.entries.is_empty() && self.bytes == 0 {
            self.base = (cols, rows);
        } else {
            self.entries.push_back(Kept::Size(cols, rows));
        }
    }

    /// The oldest half gone, then up to the next prompt (or line), so the
    /// replay starts at the start of something.
    fn trim(&mut self) {
        while self.bytes > CAP / 2 {
            match self.entries.pop_front() {
                Some(Kept::Bytes(b)) => self.bytes -= b.len(),
                Some(Kept::Size(c, r)) => self.base = (c, r),
                None => break,
            }
        }
        const PROMPT: &[u8] = b"\x1b]133;A";
        let mut cut = None;
        for (i, e) in self.entries.iter().enumerate() {
            if let Kept::Bytes(b) = e {
                if let Some(at) = b.windows(PROMPT.len()).position(|w| w == PROMPT) {
                    cut = Some((i, at));
                    break;
                }
            }
        }
        let cut = cut.or_else(|| {
            self.entries.iter().enumerate().find_map(|(i, e)| match e {
                Kept::Bytes(b) => b.iter().position(|c| *c == b'\n').map(|at| (i, at + 1)),
                _ => None,
            })
        });
        let Some((i, at)) = cut else { return };
        for _ in 0..i {
            match self.entries.pop_front() {
                Some(Kept::Bytes(b)) => self.bytes -= b.len(),
                Some(Kept::Size(c, r)) => self.base = (c, r),
                None => {}
            }
        }
        if let Some(Kept::Bytes(b)) = self.entries.front_mut() {
            b.drain(..at);
            self.bytes -= at;
        }
    }

    fn encode(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(self.bytes + 64);
        let size = |o: &mut Vec<u8>, (c, r): (u16, u16)| {
            o.push(1);
            o.extend_from_slice(&c.to_le_bytes());
            o.extend_from_slice(&r.to_le_bytes());
        };
        size(&mut o, self.base);
        for e in &self.entries {
            match e {
                Kept::Bytes(b) => {
                    o.push(0);
                    o.extend_from_slice(&(b.len() as u32).to_le_bytes());
                    o.extend_from_slice(b);
                }
                Kept::Size(c, r) => size(&mut o, (*c, *r)),
            }
        }
        o
    }
}

struct Pane {
    pty: Local,
    keys: Sender<Vec<u8>>,
    cmd: String,
    log: Log,
    /// Whether it reads keys one at a time, as last told.
    raw: Option<bool>,
}

enum Ev {
    Conn(UnixStream),
    Said(u64, ToServer),
    Left(u64),
    Out(u64, Vec<u8>),
    Ended(u64),
    Tick,
}

/// `jot serve PATH`: the session, until its last pane ends.
pub fn run(path: &Path) {
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }
    let listener = match UnixListener::bind(path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("jot serve: {}: {e}", path.display());
            return;
        }
    };
    let (tx, rx) = channel::<Ev>();
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for s in listener.incoming().flatten() {
                if tx.send(Ev::Conn(s)).is_err() {
                    break;
                }
            }
        });
    }
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            while tx.send(Ev::Tick).is_ok() {
                std::thread::sleep(Duration::from_millis(100));
            }
        });
    }
    let mut panes: HashMap<u64, Pane> = HashMap::new();
    let mut conns: HashMap<u64, UnixStream> = HashMap::new();
    let mut attached: Option<u64> = None;
    let mut layout = String::new();
    let mut next_conn = 0u64;
    let started = Instant::now();
    let send = |conns: &mut HashMap<u64, UnixStream>, c: u64, m: ToClient| {
        if let Some(s) = conns.get_mut(&c) {
            if s.write_all(&m.encode()).is_err() {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
        }
    };
    while let Ok(ev) = rx.recv() {
        match ev {
            Ev::Conn(s) => {
                next_conn += 1;
                let c = next_conn;
                let Ok(mut r) = s.try_clone() else { continue };
                conns.insert(c, s);
                let tx = tx.clone();
                std::thread::spawn(move || {
                    while let Ok(f) = read_frame(&mut r) {
                        match ToServer::decode(&f) {
                            Ok(m) => {
                                if tx.send(Ev::Said(c, m)).is_err() {
                                    return;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    let _ = tx.send(Ev::Left(c));
                });
            }
            Ev::Said(c, ToServer::Hello { proto }) => {
                let why = if proto != PROTO {
                    Some(format!("this session speaks {PROTO}, the window {proto}"))
                } else if attached.is_some_and(|a| a != c) {
                    Some("another window has it".into())
                } else {
                    None
                };
                if let Some(why) = why {
                    send(&mut conns, c, ToClient::Refused { why });
                    if let Some(s) = conns.remove(&c) {
                        let _ = s.shutdown(std::net::Shutdown::Both);
                    }
                    continue;
                }
                attached = Some(c);
                let mut held: Vec<Held> = panes.iter().map(|(id, p)| Held { pane: *id, pid: p.pty.pid, cmd: p.cmd.clone(), log: p.log.encode() }).collect();
                held.sort_by_key(|h| h.pane);
                for p in panes.values_mut() {
                    p.raw = None;
                }
                send(&mut conns, c, ToClient::Welcome { layout: layout.clone(), panes: held });
            }
            Ev::Said(c, m) if attached == Some(c) => match m {
                ToServer::Spawn { pane, cmd, cwd, cols, rows, px } => {
                    let pid = match Local::spawn(&cmd, Path::new(&cwd), cols, rows, px) {
                        Ok(pty) => {
                            let pid = pty.pid;
                            let tx2 = tx.clone();
                            pty.read_on_thread(pane, move |out| {
                                let _ = tx2.send(match out {
                                    crate::pty::Output::Bytes(id, b) => Ev::Out(id, b),
                                    _ => Ev::Ended(pane),
                                });
                            });
                            let (keys, rx_keys) = channel::<Vec<u8>>();
                            let fd = unsafe { libc::dup(pty.fd) };
                            std::thread::spawn(move || {
                                while let Ok(b) = rx_keys.recv() {
                                    Local::write_fd(fd, &b);
                                }
                                unsafe { libc::close(fd) };
                            });
                            let mut log = Log::default();
                            log.size(cols, rows);
                            panes.insert(pane, Pane { pty, keys, cmd, log, raw: None });
                            pid
                        }
                        Err(e) => {
                            eprintln!("jot serve: could not start a shell: {e}");
                            -1
                        }
                    };
                    send(&mut conns, c, ToClient::Spawned { pane, pid });
                }
                ToServer::Input { pane, bytes } => {
                    if let Some(p) = panes.get(&pane) {
                        let _ = p.keys.send(bytes);
                    }
                }
                ToServer::Resize { pane, cols, rows, px } => {
                    if let Some(p) = panes.get_mut(&pane) {
                        p.pty.resize(cols, rows, px);
                        p.log.size(cols, rows);
                    }
                }
                ToServer::Close { pane } => {
                    // Dropped: its shell hung up on.
                    panes.remove(&pane);
                }
                ToServer::Layout { text } => layout = text,
                ToServer::Hello { .. } => {}
            },
            Ev::Said(..) => {}
            Ev::Left(c) => {
                conns.remove(&c);
                if attached == Some(c) {
                    attached = None;
                    if panes.is_empty() {
                        break;
                    }
                }
            }
            Ev::Out(id, b) => {
                if let Some(p) = panes.get_mut(&id) {
                    p.log.push(&b);
                    if let Some(c) = attached {
                        send(&mut conns, c, ToClient::Output { pane: id, bytes: b });
                    }
                }
            }
            Ev::Ended(id) => {
                if panes.remove(&id).is_some() {
                    if let Some(c) = attached {
                        send(&mut conns, c, ToClient::Exit { pane: id });
                    }
                }
                crate::pty::reap();
                if panes.is_empty() && attached.is_none() {
                    break;
                }
            }
            Ev::Tick => {
                crate::pty::reap();
                if let Some(c) = attached {
                    let mut changed = Vec::new();
                    for (id, p) in panes.iter_mut() {
                        let raw = p.pty.raw();
                        if p.raw != Some(raw) {
                            p.raw = Some(raw);
                            changed.push((*id, raw));
                        }
                    }
                    for (pane, raw) in changed {
                        send(&mut conns, c, ToClient::Mode { pane, raw });
                    }
                }
                // Started but never taken up: nothing to keep it.
                if attached.is_none() && panes.is_empty() && started.elapsed() > Duration::from_secs(30) {
                    break;
                }
            }
        }
    }
    let _ = std::fs::remove_file(path);
}
