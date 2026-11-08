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
pub const PROTO: u32 = 7;

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
    /// `window` names the window for as long as it is open; `again`: it
    /// is reconnecting, and wants back only the panes it had.
    Hello { proto: u32, window: u64, again: bool, want: Vec<u64> },
    /// A new pane; the session names it, in Spawned.
    Spawn { cmd: String, cwd: String, cols: u16, rows: u16, px: (u32, u32) },
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
    /// A signal to a pane's job (its foreground process group), or to its
    /// shell and all it started.
    Signal { pane: u64, sig: i32, job: bool },
    /// What it holds that this window does not show: its workspaces.
    List,
    /// Those panes, to be shown in this window (no other window has them).
    Take { panes: Vec<u64> },
    /// This window no longer shows the pane; it runs on, for any window to
    /// show again.
    Release { pane: u64 },
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
    /// What a pane's program is doing, when it changes.
    Proc { pane: u64, proc: crate::pty::Proc },
    /// For List: its workspaces this window does not show.
    Listing { workspaces: Vec<Listed> },
    /// For Take: the panes, now this window's, with what they printed.
    Taken { panes: Vec<Held> },
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
            ToServer::Hello { proto, window, again, want } => {
                o.u8(0).u32(*proto).u64(*window).u8(*again as u8).u32(want.len() as u32);
                for w in want {
                    o.u64(*w);
                }
                &mut o
            }
            ToServer::Spawn { cmd, cwd, cols, rows, px } => o.u8(1).str(cmd).str(cwd).u16(*cols).u16(*rows).u32(px.0).u32(px.1),
            ToServer::Input { pane, bytes } => o.u8(2).u64(*pane).bytes(bytes),
            ToServer::Resize { pane, cols, rows, px } => o.u8(3).u64(*pane).u16(*cols).u16(*rows).u32(px.0).u32(px.1),
            ToServer::Close { pane } => o.u8(4).u64(*pane),
            ToServer::Layout { text } => o.u8(5).str(text),
            ToServer::Tee { pane, tee } => o.u8(6).u64(*pane).tee(tee),
            ToServer::TeeStop { pane } => o.u8(7).u64(*pane),
            ToServer::Queue { pane, queue } => o.u8(8).u64(*pane).queue(queue),
            ToServer::Peek => o.u8(9),
            ToServer::Signal { pane, sig, job } => o.u8(10).u64(*pane).u32(*sig as u32).u8(*job as u8),
            ToServer::List => o.u8(11),
            ToServer::Release { pane } => o.u8(13).u64(*pane),
            ToServer::Take { panes } => {
                o.u8(12).u32(panes.len() as u32);
                for p in panes {
                    o.u64(*p);
                }
                &mut o
            }
        };
        frame(o)
    }

    fn decode(f: &[u8]) -> io::Result<ToServer> {
        let mut i = In(f);
        Ok(match i.u8()? {
            0 => {
                let (proto, window, again) = (i.u32()?, i.u64().unwrap_or(0), i.u8().unwrap_or(0) != 0);
                let n = i.u32().unwrap_or(0);
                let want = (0..n).map(|_| i.u64()).collect::<io::Result<Vec<_>>>()?;
                ToServer::Hello { proto, window, again, want }
            }
            1 => ToServer::Spawn { cmd: i.str()?, cwd: i.str()?, cols: i.u16()?, rows: i.u16()?, px: (i.u32()?, i.u32()?) },
            2 => ToServer::Input { pane: i.u64()?, bytes: i.bytes()? },
            3 => ToServer::Resize { pane: i.u64()?, cols: i.u16()?, rows: i.u16()?, px: (i.u32()?, i.u32()?) },
            4 => ToServer::Close { pane: i.u64()? },
            5 => ToServer::Layout { text: i.str()? },
            6 => ToServer::Tee { pane: i.u64()?, tee: i.tee()? },
            7 => ToServer::TeeStop { pane: i.u64()? },
            8 => ToServer::Queue { pane: i.u64()?, queue: i.queue()? },
            9 => ToServer::Peek,
            10 => ToServer::Signal { pane: i.u64()?, sig: i.u32()? as i32, job: i.u8()? != 0 },
            11 => ToServer::List,
            13 => ToServer::Release { pane: i.u64()? },
            12 => {
                let n = i.u32()?;
                ToServer::Take { panes: (0..n).map(|_| i.u64()).collect::<io::Result<Vec<_>>>()? }
            }
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
            ToClient::Proc { pane, proc } => {
                o.u8(10).u64(*pane).str(&proc.cwd).str(&proc.fg).u32(proc.pgrp as u32).u8(proc.busy as u8);
            }
            ToClient::Listing { workspaces } => {
                o.u8(11).u32(workspaces.len() as u32);
                for w in workspaces {
                    o.str(&w.spec).u32(w.panes.len() as u32);
                    for p in &w.panes {
                        o.u64(*p);
                    }
                    o.u8(w.elsewhere as u8).str(&w.label).u8(w.running as u8);
                }
            }
            ToClient::Taken { panes } => {
                o.u8(12).u32(panes.len() as u32);
                for p in panes {
                    o.u64(p.pane).u32(p.pid as u32).str(&p.cmd).bytes(&p.log);
                    match &p.tee {
                        Some(t) => o.u8(1).tee(t),
                        None => o.u8(0),
                    };
                    o.queue(&p.queue);
                }
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
            10 => ToClient::Proc { pane: i.u64()?, proc: crate::pty::Proc { cwd: i.str()?, fg: i.str()?, pgrp: i.u32()? as i32, busy: i.u8()? != 0 } },
            11 => {
                let n = i.u32()?;
                let mut workspaces = Vec::new();
                for _ in 0..n {
                    let spec = i.str()?;
                    let k = i.u32()?;
                    let panes = (0..k).map(|_| i.u64()).collect::<io::Result<Vec<_>>>()?;
                    workspaces.push(Listed { spec, panes, elsewhere: i.u8()? != 0, label: i.str()?, running: i.u8()? != 0 });
                }
                ToClient::Listing { workspaces }
            }
            12 => {
                let n = i.u32()?;
                let mut panes = Vec::new();
                for _ in 0..n {
                    let (pane, pid, cmd, log) = (i.u64()?, i.u32()? as i32, i.str()?, i.bytes()?);
                    let tee = if i.u8()? != 0 { Some(i.tee()?) } else { None };
                    panes.push(Held { pane, pid, cmd, log, tee, queue: i.queue()? });
                }
                ToClient::Taken { panes }
            }
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

/// The machine's session's socket.
pub fn socket() -> PathBuf {
    dir().join("session.sock")
}

/// Sessions left by older versions of jot, one socket each, by number.
fn old_sessions() -> Vec<(u32, PathBuf)> {
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

/// What the window's side hears from the session, handed on to it.
pub enum Heard {
    Output(u64, Vec<u8>),
    Exit(u64),
    Mode(u64, bool),
    Ran(u64, String),
    TeeLines(u64, u64),
    TeeEnd(u64, u64, String),
    Proc(u64, crate::pty::Proc),
    /// Another machine's session reached again, after the connection
    /// dropped: the panes it holds for this window.
    Back(Vec<Held>),
    /// Another machine could not be reached, and why (said once; it is
    /// tried again, unless it runs another jot).
    Failed(String),
    /// Its workspaces this window does not show.
    Listing(Vec<Listed>),
    /// Panes taken into this window.
    Taken(Vec<Held>),
    /// The connection to the session ended: on another machine, it is
    /// reached again ([`Client::reconnect`]).
    /// The session ended under the window.
    Gone,
}

/// A window's end of the session.
pub struct Client {
    writer: Mutex<Box<dyn Write + Send>>,
    spawned: Mutex<Receiver<(u64, i32)>>,
    /// The window is going but the session stays: panes dropped on the
    /// way out are left running.
    detached: AtomicBool,
    /// Output being replayed: what the terminals answer to it was
    /// answered long ago, and is not sent.
    pub mute: AtomicBool,
    /// The machine the session is on, reached by ssh; None for this one.
    pub host: Option<String>,
    /// ssh, for a session on another machine.
    ssh: Mutex<Option<std::process::Child>>,
    /// Whether the session can be reached now. A session on another
    /// machine whose connection dropped is reached again, unseen; until
    /// then, what is typed waits here.
    connected: AtomicBool,
    waiting: Mutex<Vec<(u64, Vec<u8>)>>,
    /// Where what the session says goes, kept to be given again on
    /// reconnecting.
    hear: Arc<dyn Fn(Heard) + Send + Sync>,
    /// A background attempt to reach it is going.
    trying: AtomicBool,
    /// Its panes this window has, by the session's names: asked for by
    /// name on reaching it, as none were this window's yet.
    want: Mutex<Vec<u64>>,
}

/// The session taken up: the window's end of it, the workspaces and panes
/// no other window has, and what is worth telling.
pub struct Attached {
    pub client: Arc<Client>,
    pub layout: String,
    pub panes: Vec<Held>,
    pub notes: Vec<String>,
    /// The session was started for this window: nothing ran before it.
    pub fresh: bool,
}

impl Client {
    /// The machine's session, started if it is not running. `hear` is
    /// given what it says from then on, on a thread of its own.
    pub fn open(hear: impl Fn(Heard) + Send + Sync + 'static) -> io::Result<Attached> {
        let hear: Arc<dyn Fn(Heard) + Send + Sync> = Arc::new(hear);
        let path = socket();
        let (stream, layout, panes, fresh) = match UnixStream::connect(&path) {
            Ok(s) => {
                let (s, l, p) = Self::hello(s)?;
                (s, l, p, false)
            }
            Err(_) => {
                // Not running: its socket, if left behind, goes.
                let _ = std::fs::remove_file(&path);
                let (s, l, p) = Self::start(&path)?;
                (s, l, p, true)
            }
        };
        let mut notes = Vec::new();
        let old: Vec<String> = old_sessions().into_iter().filter(|(_, p)| UnixStream::connect(p).is_ok()).map(|(n, _)| n.to_string()).collect();
        if !old.is_empty() {
            notes.push(format!("Panes from an older jot are still running (session {}). jot kill ends them.", old.join(", ")));
        }
        let (tx, rx) = channel();
        let reader = stream.try_clone()?;
        let h = hear.clone();
        std::thread::Builder::new().name("mux".into()).spawn(move || listen(Box::new(reader), tx, h))?;
        let client = Client { writer: Mutex::new(Box::new(stream)), spawned: Mutex::new(rx), detached: AtomicBool::new(false), mute: AtomicBool::new(false), host: None, ssh: Mutex::new(None), connected: AtomicBool::new(true), waiting: Mutex::new(Vec::new()), hear, trying: AtomicBool::new(false), want: Mutex::new(Vec::new()) };
        Ok(Attached { client: Arc::new(client), layout, panes, notes, fresh })
    }

    fn hello(mut s: UnixStream) -> io::Result<(UnixStream, String, Vec<Held>)> {
        s.set_read_timeout(Some(Duration::from_secs(5)))?;
        // The quake window comes back for its own panes; any other window
        // takes up those no window has.
        s.write_all(&ToServer::Hello { proto: PROTO, window: window_id(), again: crate::quake_mode(), want: Vec::new() }.encode())?;
        let f = read_frame(&mut s)?;
        s.set_read_timeout(None)?;
        match ToClient::decode(&f) {
            Ok(ToClient::Welcome { layout, panes }) => Ok((s, layout, panes)),
            Ok(ToClient::Refused { why }) => Err(io::Error::new(io::ErrorKind::Unsupported, format!("the session running is from another version of jot ({why}); jot kill ends it"))),
            _ => Err(io::Error::new(io::ErrorKind::Unsupported, "the session running is from another version of jot; jot kill ends it")),
        }
    }

    /// The session's server started, apart from this window so that it
    /// outlives it.
    fn start(path: &std::path::Path) -> io::Result<(UnixStream, String, Vec<Held>)> {
        std::fs::create_dir_all(dir())?;
        server::launch(path)?;
        for _ in 0..200 {
            if let Ok(s) = UnixStream::connect(path) {
                return Self::hello(s);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Err(io::Error::new(io::ErrorKind::TimedOut, "the session did not start"))
    }

    fn send(&self, m: ToServer) {
        if !self.connected.load(Ordering::Acquire) {
            // Keys wait for the connection (a little of them); the rest is
            // said again, as it is then, on reconnecting.
            if let (ToServer::Input { pane, bytes }, Ok(mut w)) = (m, self.waiting.lock()) {
                if w.iter().map(|(_, b)| b.len()).sum::<usize>() < 64 << 10 {
                    w.push((pane, bytes));
                }
            }
            return;
        }
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(&m.encode());
        }
    }

    /// A pane started in the session: the name it gave it, and its
    /// shell's process id.
    pub fn spawn(&self, cmd: &str, cwd: &std::path::Path, cols: u16, rows: u16, px: (u32, u32)) -> io::Result<(u64, i32)> {
        let rx = self.spawned.lock().map_err(|_| io::Error::other("poisoned"))?;
        self.send(ToServer::Spawn { cmd: cmd.into(), cwd: cwd.display().to_string(), cols, rows, px });
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok((pane, pid)) if pid > 0 => Ok((pane, pid)),
            Ok(_) => Err(io::Error::other("could not start a shell")),
            Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "the session did not answer")),
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

    pub fn signal(&self, pane: u64, sig: i32, job: bool) {
        self.send(ToServer::Signal { pane, sig, job });
    }

    pub fn list(&self) {
        self.send(ToServer::List);
    }

    pub fn take(&self, panes: Vec<u64>) {
        self.send(ToServer::Take { panes });
    }

    /// The pane no longer shown here; it runs on.
    pub fn release(&self, pane: u64) {
        if !self.detached.load(Ordering::Acquire) {
            self.send(ToServer::Release { pane });
        }
    }

    /// The window going, its session left running.
    pub fn detach(&self) {
        self.detached.store(true, Ordering::Release);
    }
}

fn listen(mut r: Box<dyn Read + Send>, spawned: Sender<(u64, i32)>, hear: Arc<dyn Fn(Heard) + Send + Sync>) {
    loop {
        let Ok(f) = read_frame(&mut r) else { break };
        match ToClient::decode(&f) {
            Ok(ToClient::Output { pane, bytes }) => hear(Heard::Output(pane, bytes)),
            Ok(ToClient::Exit { pane }) => hear(Heard::Exit(pane)),
            Ok(ToClient::Mode { pane, raw }) => hear(Heard::Mode(pane, raw)),
            Ok(ToClient::Ran { pane, cmd }) => hear(Heard::Ran(pane, cmd)),
            Ok(ToClient::TeeLines { pane, lines }) => hear(Heard::TeeLines(pane, lines)),
            Ok(ToClient::TeeEnd { pane, lines, error }) => hear(Heard::TeeEnd(pane, lines, error)),
            Ok(ToClient::Proc { pane, proc }) => hear(Heard::Proc(pane, proc)),
            Ok(ToClient::Listing { workspaces }) => hear(Heard::Listing(workspaces)),
            Ok(ToClient::Taken { panes }) => hear(Heard::Taken(panes)),
            Ok(ToClient::Spawned { pane, pid }) => {
                let _ = spawned.send((pane, pid));
            }
            _ => {}
        }
    }
    hear(Heard::Gone);
}

/// The process at the other end of a socket: a session's server,
/// whatever its version.
fn server_pid(path: &std::path::Path) -> Option<i32> {
    use std::os::fd::AsRawFd;
    let s = UnixStream::connect(path).ok()?;
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let ok = unsafe { libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut libc::c_void, &mut len) } == 0;
    (ok && cred.pid > 0).then_some(cred.pid)
}

/// What the session holds, asked without taking anything up.
fn peek() -> io::Result<ToClient> {
    let mut s = UnixStream::connect(socket())?;
    s.set_read_timeout(Some(Duration::from_secs(2)))?;
    s.write_all(&ToServer::Peek.encode())?;
    ToClient::decode(&read_frame(&mut s)?)
}

/// `jot status`: the session, what it holds and whether a window shows
/// it. `jot kill`: it ended, every pane with it, and any left by older
/// versions of jot.
pub fn command(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("status") => match peek() {
            Ok(ToClient::Info { attached, panes, names }) => {
                let panes = if panes == 1 { "1 pane".to_string() } else { format!("{panes} panes") };
                let shown = if attached { "in a window" } else { "no window" };
                println!("{panes} · {shown}{}", if names.is_empty() { String::new() } else { format!(" · {names}") });
                0
            }
            Ok(_) => {
                println!("The session running is from another version of jot (jot kill ends it).");
                0
            }
            Err(_) => {
                println!("No session running.");
                0
            }
        },
        Some("kill") => {
            let mut ended = 0;
            for path in std::iter::once(socket()).chain(old_sessions().into_iter().map(|(_, p)| p)) {
                if let Some(pid) = server_pid(&path) {
                    unsafe { libc::kill(pid, libc::SIGTERM) };
                    ended += 1;
                }
                let _ = std::fs::remove_file(&path);
            }
            println!("{}", if ended == 0 { "No session running.".to_string() } else { "The session ended, and its panes.".to_string() });
            0
        }
        _ => 2,
    }
}

/// A session on another machine reached: ssh's ends, ssh itself, and what
/// the session holds for this window.
type Reached = (std::process::ChildStdin, std::process::ChildStdout, std::process::Child, String, Vec<Held>);

/// ssh to `host`, `jot bridge` there, and hello.
fn reach(host: &str, again: bool, want: Vec<u64>) -> io::Result<Reached> {
    use std::process::{Command, Stdio};
    // jot where the user put it: on the PATH, ~/.local/bin, or mise's.
    let remote = "exec env PATH=\"$HOME/.local/bin:$HOME/.local/share/mise/shims:$HOME/.cargo/bin:$PATH\" jot bridge";
    // Alive checks every 5 s, two missed: a dead connection is known in
    // about ten seconds, and reached again.
    let mut child = Command::new("ssh")
        .args(["-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "-o", "ServerAliveInterval=5", "-o", "ServerAliveCountMax=2", host, remote])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut w = child.stdin.take().unwrap();
    let mut r = child.stdout.take().unwrap();
    // What ssh or the far side says when it fails, kept for the error.
    let said = Arc::new(Mutex::new(String::new()));
    if let Some(mut e) = child.stderr.take() {
        let said = said.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = e.read(&mut buf) {
                if n == 0 {
                    break;
                }
                if let Ok(mut s) = said.lock() {
                    s.push_str(&String::from_utf8_lossy(&buf[..n]));
                }
            }
        });
    }
    let failed = |e: io::Error| {
        std::thread::sleep(Duration::from_millis(200));
        let s = said.lock().map(|s| s.trim().to_string()).unwrap_or_default();
        io::Error::new(e.kind(), if s.is_empty() { format!("{host}: {e}") } else { format!("{host}: {s}") })
    };
    let hello = w.write_all(&ToServer::Hello { proto: PROTO, window: window_id(), again, want }.encode()).and_then(|_| read_frame(&mut r));
    let f = match hello {
        Ok(f) => f,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(failed(e));
        }
    };
    match ToClient::decode(&f) {
        Ok(ToClient::Welcome { layout, panes }) => Ok((w, r, child, layout, panes)),
        Ok(ToClient::Refused { why }) => Err(io::Error::new(io::ErrorKind::Unsupported, format!("{host} runs another version of jot ({why})"))),
        _ => Err(io::Error::new(io::ErrorKind::Unsupported, format!("{host} runs another version of jot"))),
    }
}

impl Client {
    /// The session on another machine, reached by ssh: `jot bridge` there
    /// passes what is said between ssh and that machine's session,
    /// starting it if it is not running.
    pub fn open_remote(host: &str, hear: impl Fn(Heard) + Send + Sync + 'static) -> io::Result<Attached> {
        let hear: Arc<dyn Fn(Heard) + Send + Sync> = Arc::new(hear);
        let (w, r, child, layout, panes) = reach(host, false, Vec::new())?;
        let (tx, rx) = channel();
        let h = hear.clone();
        std::thread::Builder::new().name("mux".into()).spawn(move || listen(Box::new(r), tx, h))?;
        let client = Client { writer: Mutex::new(Box::new(w)), spawned: Mutex::new(rx), detached: AtomicBool::new(false), mute: AtomicBool::new(false), host: Some(host.to_string()), ssh: Mutex::new(Some(child)), connected: AtomicBool::new(true), waiting: Mutex::new(Vec::new()), hear, trying: AtomicBool::new(false), want: Mutex::new(Vec::new()) };
        Ok(Attached { client: Arc::new(client), layout, panes, notes: Vec::new(), fresh: false })
    }

    /// The connection to another machine dropped: reached again in the
    /// background, a second after, then longer, up to ten. What it holds
    /// then is told (Heard::Back); keys typed meanwhile follow.
    pub fn reconnect(self: &Arc<Self>) {
        if !self.connected.swap(false, Ordering::AcqRel) {
            return;
        }
        if let Some(mut c) = self.ssh.lock().ok().and_then(|mut c| c.take()) {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.retry(true, Duration::from_secs(1));
    }

    /// Another machine's session, to be reached in the background: the
    /// window goes on meanwhile, and is told (Heard::Back) when it is,
    /// or (Heard::Failed) why not, the first time it is not.
    pub fn lazy(host: &str, hear: impl Fn(Heard) + Send + Sync + 'static) -> Arc<Client> {
        let (_tx, rx) = channel();
        let me = Arc::new(Client { writer: Mutex::new(Box::new(io::sink())), spawned: Mutex::new(rx), detached: AtomicBool::new(false), mute: AtomicBool::new(false), host: Some(host.to_string()), ssh: Mutex::new(None), connected: AtomicBool::new(false), waiting: Mutex::new(Vec::new()), hear: Arc::new(hear), trying: AtomicBool::new(false), want: Mutex::new(Vec::new()) });
        me.retry(true, Duration::ZERO);
        me
    }

    /// A pane of its session this window shows, to be asked for.
    pub fn want(&self, pane: u64) {
        if let Ok(mut w) = self.want.lock() {
            if !w.contains(&pane) {
                w.push(pane);
            }
        }
    }

    /// Tried again, after it failed and gave up (another /connect to it).
    pub fn try_again(self: &Arc<Self>) {
        if !self.connected() {
            self.detached.store(false, Ordering::Release);
            self.retry(true, Duration::ZERO);
        }
    }

    /// Stops trying to reach it.
    pub fn give_up(&self) {
        self.detached.store(true, Ordering::Release);
    }

    /// Reached in the background: after `first`, then waits doubling to
    /// ten seconds. `again`: it had been reached, and wants back only the
    /// panes it had.
    fn retry(self: &Arc<Self>, again: bool, first: Duration) {
        let Some(host) = self.host.clone() else { return };
        if self.trying.swap(true, Ordering::AcqRel) {
            return;
        }
        let me = self.clone();
        std::thread::spawn(move || {
            let mut wait = first;
            let mut told = false;
            loop {
                if me.detached.load(Ordering::Acquire) {
                    me.trying.store(false, Ordering::Release);
                    return;
                }
                std::thread::sleep(wait);
                let want = me.want.lock().map(|w| w.clone()).unwrap_or_default();
                match reach(&host, again, want) {
                    Ok((w, r, child, _layout, panes)) => {
                        let (tx, rx) = channel();
                        if let (Ok(mut wr), Ok(mut sp), Ok(mut ssh)) = (me.writer.lock(), me.spawned.lock(), me.ssh.lock()) {
                            *wr = Box::new(w);
                            *sp = rx;
                            *ssh = Some(child);
                        }
                        let h = me.hear.clone();
                        let _ = std::thread::Builder::new().name("mux".into()).spawn(move || listen(Box::new(r), tx, h));
                        // Told what it holds before keys typed meanwhile go,
                        // so the window has its panes back first.
                        (me.hear)(Heard::Back(panes));
                        me.connected.store(true, Ordering::Release);
                        let keys = me.waiting.lock().map(|mut w| std::mem::take(&mut *w)).unwrap_or_default();
                        for (pane, bytes) in keys {
                            me.send(ToServer::Input { pane, bytes });
                        }
                        me.trying.store(false, Ordering::Release);
                        return;
                    }
                    Err(e) if e.kind() == io::ErrorKind::Unsupported => {
                        (me.hear)(Heard::Failed(e.to_string()));
                        me.trying.store(false, Ordering::Release);
                        (me.hear)(Heard::Gone);
                        return;
                    }
                    Err(e) => {
                        if !told {
                            told = true;
                            (me.hear)(Heard::Failed(e.to_string()));
                        }
                        wait = (wait * 2).clamp(Duration::from_secs(1), Duration::from_secs(10));
                    }
                }
            }
        });
    }

    /// Whether the session can be reached now.
    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Some(mut c) = self.ssh.lock().ok().and_then(|mut c| c.take()) {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// `jot bridge`, run by ssh from a window on another machine: what comes
/// in on stdin goes to this machine's session, what it says goes out on
/// stdout, until either side ends. The session is started if it is not
/// running, and outlives the bridge.
pub fn bridge() -> i32 {
    let path = socket();
    let stream = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(_) => {
            let _ = std::fs::remove_file(&path);
            let started = std::fs::create_dir_all(dir()).and_then(|_| server::launch(&path));
            if let Err(e) = started {
                eprintln!("jot bridge: could not start the session: {e}");
                return 1;
            }
            let mut s = None;
            for _ in 0..200 {
                if let Ok(c) = UnixStream::connect(&path) {
                    s = Some(c);
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            match s {
                Some(s) => s,
                None => {
                    eprintln!("jot bridge: the session did not start");
                    return 1;
                }
            }
        }
    };
    let Ok(mut to_session) = stream.try_clone() else { return 1 };
    std::thread::spawn(move || {
        let _ = io::copy(&mut io::stdin().lock(), &mut to_session);
        let _ = to_session.shutdown(std::net::Shutdown::Both);
        std::process::exit(0);
    });
    let mut from_session = stream;
    let mut out = io::stdout().lock();
    let mut buf = vec![0u8; 64 << 10];
    loop {
        match from_session.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if out.write_all(&buf[..n]).and_then(|_| out.flush()).is_err() {
                    break;
                }
            }
        }
    }
    0
}

/// The quake window's name in the session (an odd number, as every
/// window's is).
pub const QUAKE_WINDOW: u64 = 0x6a6f_7444_726f_7001;

/// This window's name to the session, the same while it is open: what it
/// had is given back to it on reconnecting, even from a connection the
/// session has not yet seen drop.
pub fn window_id() -> u64 {
    static ID: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *ID.get_or_init(|| {
        // The quake window is always the same window, so that a new one takes
        // up the panes the last one had, and only those.
        if crate::quake_mode() {
            return QUAKE_WINDOW;
        }
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
        t ^ ((std::process::id() as u64) << 32) | 1
    })
}

/// A pane's name in the window: its machine's place among the window's
/// machines, and its name in that machine's session. This machine's
/// panes (place 0) keep their session's names.
pub fn app_id(machine: usize, pane: u64) -> u64 {
    ((machine as u64) << 40) | pane
}

/// A window's pane name as its machine and its session's name for it.
pub fn split_id(id: u64) -> (usize, u64) {
    ((id >> 40) as usize, id & ((1 << 40) - 1))
}

/// One of a session's workspaces, as a window that does not show it
/// lists it: its layout (a workspace's text in a session file), and
/// whether another window shows it.
#[derive(Clone, Debug)]
pub struct Listed {
    pub spec: String,
    pub panes: Vec<u64>,
    pub elsewhere: bool,
    /// A pane in no workspace: what it runs, or its folder; empty for a
    /// workspace.
    pub label: String,
    /// Whether it runs something now (a pane in no workspace).
    pub running: bool,
}
