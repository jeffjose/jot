//! Panes that outlive the window. Every shell runs in a `jot serve`
//! process, a session; the window is its client, over a Unix socket.
//! Closing the window leaves the session running, its shells and what they
//! run carrying on; starting jot again takes up a session no window has,
//! the panes as they were, their screens and scrollback replayed.
//!
//! The server keeps what each pane printed, as it came, with the time
//! now and then, trimmed at a prompt when it grows long, and replays it to
//! the window that attaches, so command blocks, prompts, colors and how
//! long commands took come back as they were. What must go on with no
//! window is the server's too: /tee's file, and the queue of commands
//! waiting their turn.

pub mod server;

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Bumped when the messages change: a window and a session that differ
/// do not talk. (Hello and Refused keep their shape across versions, so
/// that the difference can be told.)
pub const PROTO: u32 = 2;

/// A pane's queue as both ends keep it: commands waiting for the prompt,
/// whether a failure has held them, and those waiting on another pane's
/// command (@N).
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Queue {
    pub items: Vec<String>,
    pub paused: bool,
    pub after: Vec<(u64, String)>,
    /// Its command is being restarted (Ctrl R): its failing then is
    /// meant, and holds nothing.
    pub restarting: bool,
}

/// A pane's /tee as the session has it.
#[derive(Clone, Debug)]
pub struct TeeState {
    pub path: String,
    pub lines: u64,
    pub all: bool,
    pub armed: bool,
}

/// What the window says to the session.
pub enum ToServer {
    Hello { proto: u32 },
    Spawn { pane: u64, cmd: String, cwd: String, cols: u16, rows: u16, px: (u32, u32) },
    Input { pane: u64, bytes: Vec<u8> },
    Resize { pane: u64, cols: u16, rows: u16, px: (u32, u32) },
    Close { pane: u64 },
    /// The workspaces as they are, a session file's text, kept to be
    /// given to the next window.
    Layout { text: String },
    /// /tee: what the pane prints, to a file too.
    Tee { pane: u64, tee: TeeState },
    TeeStop { pane: u64 },
    /// The pane's queue as the window has it now.
    Queue { pane: u64, queue: Queue },
    /// What the session holds, without taking it up: `jot sessions`.
    Peek,
}

/// A pane the session has, as it says on attaching.
pub struct Held {
    pub pane: u64,
    pub pid: i32,
    pub cmd: String,
    /// What it printed, to be replayed: see [`log_entries`].
    pub log: Vec<u8>,
    pub tee: Option<TeeState>,
    pub queue: Queue,
}

/// What the session says to the window.
pub enum ToClient {
    Welcome { layout: String, panes: Vec<Held> },
    /// Another window has it, or it speaks another protocol.
    Refused { why: String },
    Spawned { pane: u64, pid: i32 },
    Output { pane: u64, bytes: Vec<u8> },
    Exit { pane: u64 },
    /// Whether the program reads keys one at a time (ICANON off): a
    /// shell at its prompt, vim. Sent when it changes.
    Mode { pane: u64, raw: bool },
    /// A queued command typed at the pane's prompt.
    Ran { pane: u64, cmd: String },
    /// How many lines a /tee has written.
    TeeLines { pane: u64, lines: u64 },
    /// A /tee ended: stopped, its command done, or the file failed.
    TeeEnd { pane: u64, lines: u64, error: String },
    /// For Peek: whether a window has it, and its workspaces and panes.
    Info { attached: bool, panes: u32, names: String },
}

// The wire: a frame is its length (u32), then a tag, then the fields.

#[derive(Default)]
struct Out(Vec<u8>);

impl Out {
    fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.u32(v.len() as u32);
        self.0.extend_from_slice(v);
        self
    }
    fn str(&mut self, v: &str) -> &mut Self {
        self.bytes(v.as_bytes())
    }
    fn queue(&mut self, q: &Queue) -> &mut Self {
        self.u32(q.items.len() as u32);
        for i in &q.items {
            self.str(i);
        }
        self.u8(q.paused as u8).u32(q.after.len() as u32);
        for (t, c) in &q.after {
            self.u64(*t).str(c);
        }
        self.u8(q.restarting as u8)
    }
    fn tee(&mut self, t: &TeeState) -> &mut Self {
        self.str(&t.path).u64(t.lines).u8(t.all as u8).u8(t.armed as u8)
    }
}

struct In<'a>(&'a [u8]);

impl In<'_> {
    fn take(&mut self, n: usize) -> io::Result<&[u8]> {
        if self.0.len() < n {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "short frame"));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn bytes(&mut self) -> io::Result<Vec<u8>> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    fn str(&mut self) -> io::Result<String> {
        Ok(String::from_utf8_lossy(&self.bytes()?).into_owned())
    }
    fn queue(&mut self) -> io::Result<Queue> {
        let n = self.u32()?;
        let items = (0..n).map(|_| self.str()).collect::<io::Result<Vec<_>>>()?;
        let paused = self.u8()? != 0;
        let n = self.u32()?;
        let after = (0..n).map(|_| Ok((self.u64()?, self.str()?))).collect::<io::Result<Vec<_>>>()?;
        Ok(Queue { items, paused, after, restarting: self.u8()? != 0 })
    }
    fn tee(&mut self) -> io::Result<TeeState> {
        Ok(TeeState { path: self.str()?, lines: self.u64()?, all: self.u8()? != 0, armed: self.u8()? != 0 })
    }
}

fn frame(out: Out) -> Vec<u8> {
    let mut f = Vec::with_capacity(out.0.len() + 4);
    f.extend_from_slice(&(out.0.len() as u32).to_le_bytes());
    f.extend_from_slice(&out.0);
    f
}

fn read_frame(r: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let mut buf = vec![0u8; u32::from_le_bytes(len) as usize];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

fn bad(t: u8) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("unknown message {t}"))
}

impl ToServer {
    fn encode(&self) -> Vec<u8> {
        let mut o = Out::default();
        match self {
            ToServer::Hello { proto } => o.u8(0).u32(*proto),
            ToServer::Spawn { pane, cmd, cwd, cols, rows, px } => o.u8(1).u64(*pane).str(cmd).str(cwd).u16(*cols).u16(*rows).u32(px.0).u32(px.1),
            ToServer::Input { pane, bytes } => o.u8(2).u64(*pane).bytes(bytes),
            ToServer::Resize { pane, cols, rows, px } => o.u8(3).u64(*pane).u16(*cols).u16(*rows).u32(px.0).u32(px.1),
            ToServer::Close { pane } => o.u8(4).u64(*pane),
            ToServer::Layout { text } => o.u8(5).str(text),
            ToServer::Tee { pane, tee } => o.u8(6).u64(*pane).tee(tee),
            ToServer::TeeStop { pane } => o.u8(7).u64(*pane),
            ToServer::Queue { pane, queue } => o.u8(8).u64(*pane).queue(queue),
            ToServer::Peek => o.u8(9),
        };
        frame(o)
    }

    fn decode(f: &[u8]) -> io::Result<ToServer> {
        let mut i = In(f);
        Ok(match i.u8()? {
            0 => ToServer::Hello { proto: i.u32()? },
            1 => ToServer::Spawn { pane: i.u64()?, cmd: i.str()?, cwd: i.str()?, cols: i.u16()?, rows: i.u16()?, px: (i.u32()?, i.u32()?) },
            2 => ToServer::Input { pane: i.u64()?, bytes: i.bytes()? },
            3 => ToServer::Resize { pane: i.u64()?, cols: i.u16()?, rows: i.u16()?, px: (i.u32()?, i.u32()?) },
            4 => ToServer::Close { pane: i.u64()? },
            5 => ToServer::Layout { text: i.str()? },
            6 => ToServer::Tee { pane: i.u64()?, tee: i.tee()? },
            7 => ToServer::TeeStop { pane: i.u64()? },
            8 => ToServer::Queue { pane: i.u64()?, queue: i.queue()? },
            9 => ToServer::Peek,
            t => return Err(bad(t)),
        })
    }
}

impl ToClient {
    fn encode(&self) -> Vec<u8> {
        let mut o = Out::default();
        match self {
            ToClient::Welcome { layout, panes } => {
                o.u8(0).str(layout).u32(panes.len() as u32);
                for p in panes {
                    o.u64(p.pane).u32(p.pid as u32).str(&p.cmd).bytes(&p.log);
                    match &p.tee {
                        Some(t) => o.u8(1).tee(t),
                        None => o.u8(0),
                    };
                    o.queue(&p.queue);
                }
            }
            ToClient::Refused { why } => {
                o.u8(1).str(why);
            }
            ToClient::Spawned { pane, pid } => {
                o.u8(2).u64(*pane).u32(*pid as u32);
            }
            ToClient::Output { pane, bytes } => {
                o.u8(3).u64(*pane).bytes(bytes);
            }
            ToClient::Exit { pane } => {
                o.u8(4).u64(*pane);
            }
            ToClient::Mode { pane, raw } => {
                o.u8(5).u64(*pane).u8(*raw as u8);
            }
            ToClient::Ran { pane, cmd } => {
                o.u8(6).u64(*pane).str(cmd);
            }
            ToClient::TeeLines { pane, lines } => {
                o.u8(7).u64(*pane).u64(*lines);
            }
            ToClient::TeeEnd { pane, lines, error } => {
                o.u8(8).u64(*pane).u64(*lines).str(error);
            }
            ToClient::Info { attached, panes, names } => {
                o.u8(9).u8(*attached as u8).u32(*panes).str(names);
            }
        }
        frame(o)
    }

    fn decode(f: &[u8]) -> io::Result<ToClient> {
        let mut i = In(f);
        Ok(match i.u8()? {
            0 => {
                let layout = i.str()?;
                let n = i.u32()?;
                let mut panes = Vec::new();
                for _ in 0..n {
                    let (pane, pid, cmd, log) = (i.u64()?, i.u32()? as i32, i.str()?, i.bytes()?);
                    let tee = if i.u8()? != 0 { Some(i.tee()?) } else { None };
                    panes.push(Held { pane, pid, cmd, log, tee, queue: i.queue()? });
                }
                ToClient::Welcome { layout, panes }
            }
            1 => ToClient::Refused { why: i.str()? },
            2 => ToClient::Spawned { pane: i.u64()?, pid: i.u32()? as i32 },
            3 => ToClient::Output { pane: i.u64()?, bytes: i.bytes()? },
            4 => ToClient::Exit { pane: i.u64()? },
            5 => ToClient::Mode { pane: i.u64()?, raw: i.u8()? != 0 },
            6 => ToClient::Ran { pane: i.u64()?, cmd: i.str()? },
            7 => ToClient::TeeLines { pane: i.u64()?, lines: i.u64()? },
            8 => ToClient::TeeEnd { pane: i.u64()?, lines: i.u64()?, error: i.str()? },
            9 => ToClient::Info { attached: i.u8()? != 0, panes: i.u32()?, names: i.str()? },
            t => return Err(bad(t)),
        })
    }
}

/// A pane's log, as the server keeps it and the window replays it: what
/// it printed, the sizes it was made along the way, and the time now and
/// then (milliseconds since 1970).
pub enum Entry<'a> {
    Bytes(&'a [u8]),
    Size(u16, u16),
    Time(u64),
}

pub fn log_entries(mut log: &[u8]) -> impl Iterator<Item = Entry<'_>> {
    std::iter::from_fn(move || {
        let (&tag, rest) = log.split_first()?;
        match tag {
            0 if rest.len() >= 4 => {
                let n = u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize;
                let body = rest.get(4..4 + n)?;
                log = &rest[4 + n..];
                Some(Entry::Bytes(body))
            }
            1 if rest.len() >= 4 => {
                let cols = u16::from_le_bytes(rest[..2].try_into().unwrap());
                let rows = u16::from_le_bytes(rest[2..4].try_into().unwrap());
                log = &rest[4..];
                Some(Entry::Size(cols, rows))
            }
            2 if rest.len() >= 8 => {
                let ms = u64::from_le_bytes(rest[..8].try_into().unwrap());
                log = &rest[8..];
                Some(Entry::Time(ms))
            }
            _ => None,
        }
    })
}

/// Now, in milliseconds since 1970: the log's clock, which outlives the
/// processes that keep it.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Where sessions' sockets are: one each, named by number.
pub fn dir() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join(format!("jot-{}", unsafe { libc::getuid() }))
}

/// The sessions there are, by number, lowest first.
fn sessions() -> Vec<(u32, PathBuf)> {
    let mut found: Vec<(u32, PathBuf)> = std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let n = name.strip_suffix(".sock")?.parse().ok()?;
            Some((n, e.path()))
        })
        .collect();
    found.sort();
    found
}

/// Which session a window takes up.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Pick {
    /// The first no window has, else a new one.
    Free,
    /// This one.
    Number(u32),
    New,
}

/// What the window's side hears from the session, handed on to it.
pub enum Heard {
    Output(u64, Vec<u8>),
    Exit(u64),
    Mode(u64, bool),
    Ran(u64, String),
    TeeLines(u64, u64),
    TeeEnd(u64, u64, String),
    /// The session ended under the window.
    Gone,
}

/// The window's end of a session.
pub struct Client {
    writer: Mutex<UnixStream>,
    spawned: Mutex<Receiver<(u64, i32)>>,
    /// The window is going but the session stays: panes dropped on the
    /// way out are left running.
    detached: AtomicBool,
    /// Output being replayed: what the terminals answer to it was
    /// answered long ago, and is not sent.
    pub mute: AtomicBool,
    /// The session's number, as its socket is named.
    #[allow(dead_code)]
    pub name: u32,
}

/// A session taken up: its client, what it held, and what is worth
/// telling about the others.
pub struct Attached {
    pub client: Arc<Client>,
    pub layout: String,
    pub panes: Vec<Held>,
    pub notes: Vec<String>,
}

type Found = (UnixStream, u32, String, Vec<Held>);

impl Client {
    /// Takes up the session asked for. `hear` is given what the session
    /// says from then on, on a thread of its own.
    pub fn open(pick: Pick, hear: impl Fn(Heard) + Send + 'static) -> io::Result<Attached> {
        let mut notes = Vec::new();
        let (stream, name, layout, panes) = match pick {
            Pick::Free => Self::find(&mut notes).or_else(|_| Self::start())?,
            Pick::New => Self::start()?,
            Pick::Number(n) => {
                let path = dir().join(format!("{n}.sock"));
                let (s, layout, panes) = Self::hello(UnixStream::connect(&path).map_err(|e| io::Error::new(e.kind(), format!("no session {n}")))?)?;
                (s, n, layout, panes)
            }
        };
        // Others no window has: said, so they are not forgotten.
        let free: Vec<String> = list().into_iter().filter(|s| s.number != name && !s.attached && s.error.is_empty()).map(|s| s.number.to_string()).collect();
        if !free.is_empty() {
            notes.push(format!("Also running, with no window: session {} (jot attach N)", free.join(", ")));
        }
        let (tx, rx) = channel();
        let reader = stream.try_clone()?;
        std::thread::Builder::new().name("mux".into()).spawn(move || listen(reader, tx, hear))?;
        Ok(Attached { client: Arc::new(Client { writer: Mutex::new(stream), spawned: Mutex::new(rx), detached: AtomicBool::new(false), mute: AtomicBool::new(false), name }), layout, panes, notes })
    }

    /// The first session that takes this window.
    fn find(notes: &mut Vec<String>) -> io::Result<Found> {
        for (n, path) in sessions() {
            match UnixStream::connect(&path) {
                Ok(s) => match Self::hello(s) {
                    Ok((s, layout, panes)) => return Ok((s, n, layout, panes)),
                    Err(e) if e.kind() == io::ErrorKind::Unsupported => notes.push(format!("Session {n} is from another version of jot; its panes run on there. jot kill {n} ends it.")),
                    Err(_) => {}
                },
                Err(_) => {
                    // Gone: its socket left behind.
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        Err(io::Error::new(io::ErrorKind::NotFound, "no session free"))
    }

    fn hello(mut s: UnixStream) -> io::Result<(UnixStream, String, Vec<Held>)> {
        s.set_read_timeout(Some(Duration::from_secs(5)))?;
        s.write_all(&ToServer::Hello { proto: PROTO }.encode())?;
        let f = read_frame(&mut s)?;
        s.set_read_timeout(None)?;
        match ToClient::decode(&f) {
            Ok(ToClient::Welcome { layout, panes }) => Ok((s, layout, panes)),
            Ok(ToClient::Refused { why }) if why.starts_with("proto") || why.contains(" speaks ") => Err(io::Error::new(io::ErrorKind::Unsupported, why)),
            Ok(ToClient::Refused { why }) => Err(io::Error::other(why)),
            // A welcome in another version's shape.
            _ => Err(io::Error::new(io::ErrorKind::Unsupported, "another version")),
        }
    }

    /// A new session, its server started apart from this window so that
    /// it outlives it.
    fn start() -> io::Result<Found> {
        let taken: Vec<u32> = sessions().into_iter().map(|(n, _)| n).collect();
        let n = (1..).find(|n| !taken.contains(n)).unwrap();
        std::fs::create_dir_all(dir())?;
        let path = dir().join(format!("{n}.sock"));
        server::launch(&path)?;
        for _ in 0..200 {
            if let Ok(s) = UnixStream::connect(&path) {
                let (s, layout, panes) = Self::hello(s)?;
                return Ok((s, n, layout, panes));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Err(io::Error::new(io::ErrorKind::TimedOut, "the session did not start"))
    }

    fn send(&self, m: ToServer) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(&m.encode());
        }
    }

    /// A pane started in the session; its shell's process id.
    pub fn spawn(&self, pane: u64, cmd: &str, cwd: &std::path::Path, cols: u16, rows: u16, px: (u32, u32)) -> io::Result<i32> {
        let rx = self.spawned.lock().map_err(|_| io::Error::other("poisoned"))?;
        self.send(ToServer::Spawn { pane, cmd: cmd.into(), cwd: cwd.display().to_string(), cols, rows, px });
        loop {
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok((p, pid)) if p == pane => return if pid > 0 { Ok(pid) } else { Err(io::Error::other("could not start a shell")) },
                Ok(_) => continue,
                Err(_) => return Err(io::Error::new(io::ErrorKind::TimedOut, "the session did not answer")),
            }
        }
    }

    pub fn input(&self, pane: u64, bytes: &[u8]) {
        self.send(ToServer::Input { pane, bytes: bytes.to_vec() });
    }

    pub fn resize(&self, pane: u64, cols: u16, rows: u16, px: (u32, u32)) {
        self.send(ToServer::Resize { pane, cols, rows, px });
    }

    pub fn close(&self, pane: u64) {
        if !self.detached.load(Ordering::Acquire) {
            self.send(ToServer::Close { pane });
        }
    }

    pub fn layout(&self, text: String) {
        self.send(ToServer::Layout { text });
    }

    pub fn tee(&self, pane: u64, tee: TeeState) {
        self.send(ToServer::Tee { pane, tee });
    }

    pub fn tee_stop(&self, pane: u64) {
        self.send(ToServer::TeeStop { pane });
    }

    pub fn queue(&self, pane: u64, queue: Queue) {
        self.send(ToServer::Queue { pane, queue });
    }

    /// The window going, its session left running.
    pub fn detach(&self) {
        self.detached.store(true, Ordering::Release);
    }
}

fn listen(mut r: UnixStream, spawned: Sender<(u64, i32)>, hear: impl Fn(Heard)) {
    loop {
        let Ok(f) = read_frame(&mut r) else { break };
        match ToClient::decode(&f) {
            Ok(ToClient::Output { pane, bytes }) => hear(Heard::Output(pane, bytes)),
            Ok(ToClient::Exit { pane }) => hear(Heard::Exit(pane)),
            Ok(ToClient::Mode { pane, raw }) => hear(Heard::Mode(pane, raw)),
            Ok(ToClient::Ran { pane, cmd }) => hear(Heard::Ran(pane, cmd)),
            Ok(ToClient::TeeLines { pane, lines }) => hear(Heard::TeeLines(pane, lines)),
            Ok(ToClient::TeeEnd { pane, lines, error }) => hear(Heard::TeeEnd(pane, lines, error)),
            Ok(ToClient::Spawned { pane, pid }) => {
                let _ = spawned.send((pane, pid));
            }
            _ => {}
        }
    }
    hear(Heard::Gone);
}

/// A session, as `jot sessions` lists it.
pub struct Listed {
    pub number: u32,
    pub attached: bool,
    pub panes: u32,
    pub names: String,
    /// Why it could not be asked: another version, mostly.
    pub error: String,
}

/// The sessions running, asked what they hold.
pub fn list() -> Vec<Listed> {
    let mut out = Vec::new();
    for (n, path) in sessions() {
        let Ok(mut s) = UnixStream::connect(&path) else {
            let _ = std::fs::remove_file(&path);
            continue;
        };
        // Peek, so that asking does not take the session up.
        let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
        let info = s.write_all(&ToServer::Peek.encode()).and_then(|_| read_frame(&mut s)).and_then(|f| ToClient::decode(&f));
        match info {
            Ok(ToClient::Info { attached, panes, names }) => out.push(Listed { number: n, attached, panes, names, error: String::new() }),
            _ => out.push(Listed { number: n, attached: false, panes: 0, names: String::new(), error: "another version of jot".into() }),
        }
    }
    out
}

/// The process at the other end of a session's socket: its server,
/// whatever its version.
fn server_pid(n: u32) -> Option<i32> {
    use std::os::fd::AsRawFd;
    let s = UnixStream::connect(dir().join(format!("{n}.sock"))).ok()?;
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let ok = unsafe { libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut libc::c_void, &mut len) } == 0;
    (ok && cred.pid > 0).then_some(cred.pid)
}

/// `jot sessions`, `jot kill N`: the sessions listed, or one ended with
/// its panes.
pub fn command(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("sessions") => {
            let all = list();
            if all.is_empty() {
                println!("No sessions running.");
            }
            for s in all {
                if !s.error.is_empty() {
                    println!("{}  ({})", s.number, s.error);
                } else {
                    let panes = if s.panes == 1 { "1 pane".to_string() } else { format!("{} panes", s.panes) };
                    println!("{}  {}  {}{}", s.number, panes, s.names, if s.attached { "  (in a window)" } else { "" });
                }
            }
            0
        }
        Some("kill") => {
            let Some(n) = args.get(1).and_then(|a| a.parse::<u32>().ok()) else {
                eprintln!("jot kill: which session? (jot sessions lists them)");
                return 2;
            };
            match server_pid(n) {
                Some(pid) => {
                    unsafe { libc::kill(pid, libc::SIGTERM) };
                    println!("Session {n} ended.");
                    0
                }
                None => {
                    eprintln!("jot kill: no session {n}");
                    1
                }
            }
        }
        _ => 2,
    }
}
