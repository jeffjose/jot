//! Panes that outlive the window. Every shell runs in a `jot serve`
//! process, a session; the window is its client, over a Unix socket.
//! Closing the window leaves the session running, its shells and what they
//! run carrying on; starting jot again takes up a session no window has,
//! the panes as they were, their screens and scrollback replayed.
//!
//! The server keeps what each pane printed, as it came, trimmed at a
//! prompt when it grows long, and replays it to the window that attaches,
//! so command blocks, prompts and colors come back as they were.

pub mod server;

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Bumped when the messages change: a window and a session that differ
/// do not talk.
pub const PROTO: u32 = 1;

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
}

/// A pane the session has, as it says on attaching.
pub struct Held {
    pub pane: u64,
    pub pid: i32,
    pub cmd: String,
    /// What it printed, to be replayed: see [`Log`].
    pub log: Vec<u8>,
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
            t => return Err(io::Error::new(io::ErrorKind::InvalidData, format!("unknown message {t}"))),
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
                    panes.push(Held { pane: i.u64()?, pid: i.u32()? as i32, cmd: i.str()?, log: i.bytes()? });
                }
                ToClient::Welcome { layout, panes }
            }
            1 => ToClient::Refused { why: i.str()? },
            2 => ToClient::Spawned { pane: i.u64()?, pid: i.u32()? as i32 },
            3 => ToClient::Output { pane: i.u64()?, bytes: i.bytes()? },
            4 => ToClient::Exit { pane: i.u64()? },
            5 => ToClient::Mode { pane: i.u64()?, raw: i.u8()? != 0 },
            t => return Err(io::Error::new(io::ErrorKind::InvalidData, format!("unknown message {t}"))),
        })
    }
}

/// A pane's log, as the server keeps it and the window replays it: what
/// it printed, and the sizes it was made along the way.
pub enum Entry<'a> {
    Bytes(&'a [u8]),
    Size(u16, u16),
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
            _ => None,
        }
    })
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

/// What the window's side hears from the session, handed on to it.
pub enum Heard {
    Output(u64, Vec<u8>),
    Exit(u64),
    Mode(u64, bool),
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

/// A session taken up: its client, and what it held.
pub struct Attached {
    pub client: Arc<Client>,
    pub layout: String,
    pub panes: Vec<Held>,
}

impl Client {
    /// Takes up a session no window has, or starts one. `hear` is given
    /// what the session says from then on, on a thread of its own.
    pub fn open(hear: impl Fn(Heard) + Send + 'static) -> io::Result<Attached> {
        let (stream, name, layout, panes) = Self::find().or_else(|_| Self::start())?;
        let (tx, rx) = channel();
        let reader = stream.try_clone()?;
        std::thread::Builder::new().name("mux".into()).spawn(move || listen(reader, tx, hear))?;
        Ok(Attached { client: Arc::new(Client { writer: Mutex::new(stream), spawned: Mutex::new(rx), detached: AtomicBool::new(false), mute: AtomicBool::new(false), name }), layout, panes })
    }

    /// The first session that takes this window.
    fn find() -> io::Result<(UnixStream, u32, String, Vec<Held>)> {
        for (n, path) in sessions() {
            match UnixStream::connect(&path) {
                Ok(s) => match Self::hello(s) {
                    Ok((s, layout, panes)) => return Ok((s, n, layout, panes)),
                    Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
                        // Gone: its socket left behind.
                        let _ = std::fs::remove_file(&path);
                    }
                    Err(_) => {}
                },
                Err(_) => {
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
        match ToClient::decode(&f)? {
            ToClient::Welcome { layout, panes } => Ok((s, layout, panes)),
            ToClient::Refused { why } => Err(io::Error::other(why)),
            _ => Err(io::Error::new(io::ErrorKind::InvalidData, "no welcome")),
        }
    }

    /// A new session, its server started apart from this window so that
    /// it outlives it.
    fn start() -> io::Result<(UnixStream, u32, String, Vec<Held>)> {
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
            Ok(ToClient::Spawned { pane, pid }) => {
                let _ = spawned.send((pane, pid));
            }
            _ => {}
        }
    }
    hear(Heard::Gone);
}
