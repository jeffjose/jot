//! `jot serve`: a session. It starts each pane's shell on a PTY of its
//! own, keeps what it prints, and passes keys and output between the
//! shells and the windows showing them. It runs on when the windows go,
//! and ends with its last pane.
//!
//! What has to go on with no window is done here: /tee writes its file,
//! and queued commands are typed at the prompt when it comes. It watches
//! the shell's marks (OSC 133) for that: A, a prompt; C, a command
//! started; D, it ended, and how. With no window, a failed command holds
//! the queue behind it and commands waiting on it (@N) go on, as the
//! window would have done.

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

/// What a pane printed, as it came, the sizes it was made, and the time
/// now and then.
#[derive(Default)]
struct Log {
    entries: VecDeque<Kept>,
    bytes: usize,
    /// The size before the first entry kept.
    base: (u16, u16),
    /// When the time was last put down.
    timed: Option<Instant>,
}

enum Kept {
    Bytes(Vec<u8>),
    Size(u16, u16),
    Time(u64),
}

impl Log {
    fn push(&mut self, b: &[u8]) {
        // The time, a few times a second at most, so that replayed
        // commands take as long as they did.
        if self.timed.is_none_or(|t| t.elapsed() >= Duration::from_millis(250)) {
            self.timed = Some(Instant::now());
            self.entries.push_back(Kept::Time(now_ms()));
        }
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

    fn drop_front(&mut self) -> bool {
        match self.entries.pop_front() {
            Some(Kept::Bytes(b)) => self.bytes -= b.len(),
            Some(Kept::Size(c, r)) => self.base = (c, r),
            Some(Kept::Time(_)) => {}
            None => return false,
        }
        true
    }

    /// The oldest half gone, then up to the next prompt (or line), so the
    /// replay starts at the start of something.
    fn trim(&mut self) {
        while self.bytes > CAP / 2 && self.drop_front() {}
        const PROMPT: &[u8] = b"\x1b]133;A";
        let find = |entries: &VecDeque<Kept>, f: &dyn Fn(&[u8]) -> Option<usize>| entries.iter().enumerate().find_map(|(i, e)| if let Kept::Bytes(b) = e { f(b).map(|at| (i, at)) } else { None });
        let cut = find(&self.entries, &|b| b.windows(PROMPT.len()).position(|w| w == PROMPT)).or_else(|| find(&self.entries, &|b| b.iter().position(|c| *c == b'\n').map(|at| at + 1)));
        let Some((i, at)) = cut else { return };
        for _ in 0..i {
            self.drop_front();
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
                Kept::Time(ms) => {
                    o.push(2);
                    o.extend_from_slice(&ms.to_le_bytes());
                }
            }
        }
        o
    }
}

/// A pane's /tee: its output, as plain text, to a file too.
struct Tee {
    state: TeeState,
    file: std::fs::File,
    /// The count last told to the window.
    told: u64,
}

impl Tee {
    /// What of these bytes goes to the file: all of them, or only what
    /// the command printed, from its start mark to its end mark. The
    /// second is true once the command ended.
    fn take<'a>(&mut self, bytes: &'a [u8]) -> (&'a [u8], bool) {
        if self.state.all {
            return (bytes, false);
        }
        let mut rest = bytes;
        if self.state.armed {
            let Some(at) = find(rest, b"\x1b]133;C") else { return (&[], false) };
            let end = rest[at..].iter().position(|b| *b == 0x07).map_or(rest.len(), |e| at + e + 1);
            rest = &rest[end..];
            self.state.armed = false;
        }
        match find(rest, b"\x1b]133;D") {
            Some(at) => (&rest[..at], true),
            None => (rest, false),
        }
    }
}

fn find(bytes: &[u8], which: &[u8]) -> Option<usize> {
    bytes.windows(which.len()).position(|w| w == which)
}

/// A shell mark: A, B, C, or D with the exit status if given.
enum Mark {
    Prompt,
    Started,
    Ended(Option<i32>),
}

/// The marks in `bytes`, those that end past `from` (the bytes before it
/// were seen with the last read, and their marks with them).
fn marks(bytes: &[u8], from: usize) -> Vec<Mark> {
    const OSC: &[u8] = b"\x1b]133;";
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(at) = find(&bytes[i..], OSC) {
        let start = i + at + OSC.len();
        let Some(&kind) = bytes.get(start) else { break };
        i = start;
        if start < from {
            continue;
        }
        match kind {
            b'A' => out.push(Mark::Prompt),
            b'C' => out.push(Mark::Started),
            b'D' => {
                let code = bytes[start + 1..].strip_prefix(b";").map(|r| r.iter().take_while(|c| c.is_ascii_digit() || **c == b'-').map(|c| *c as char).collect::<String>()).and_then(|s| s.parse().ok());
                out.push(Mark::Ended(code));
            }
            _ => {}
        }
    }
    out
}

struct Pane {
    pty: Local,
    keys: Sender<Vec<u8>>,
    cmd: String,
    log: Log,
    /// Whether it reads keys one at a time, as last told.
    raw: Option<bool>,
    /// The last bytes read, for a mark split between reads.
    tail: Vec<u8>,
    /// At a prompt, nothing typed there yet by the queue.
    at_prompt: bool,
    queue: Queue,
    tee: Option<Tee>,
    /// What its program was doing, as last told.
    proc: Option<crate::pty::Proc>,
    /// The window it is in, if one has it.
    owner: Option<u64>,
    /// The name of the window that has it, or last had it.
    window: u64,
}

enum Ev {
    Conn(UnixStream),
    Said(u64, ToServer),
    Left(u64),
    Out(u64, Vec<u8>),
    Ended(u64),
    Tick,
}

struct Server {
    panes: HashMap<u64, Pane>,
    conns: HashMap<u64, UnixStream>,
    /// The windows: connections that said hello.
    windows: Vec<u64>,
    /// Each window's workspaces, as it last said.
    layouts: HashMap<u64, String>,
    /// Each window's name, by its connection.
    names: HashMap<u64, u64>,
    /// The workspaces of windows that went, for the next to take up, with
    /// the window's name.
    left: Vec<(u64, String)>,
    next_pane: u64,
}

impl Server {
    fn send(&mut self, c: u64, m: ToClient) {
        if let Some(s) = self.conns.get_mut(&c) {
            if s.write_all(&m.encode()).is_err() {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
        }
    }

    /// Said to the window a pane is in, if one has it.
    fn tell(&mut self, pane: u64, m: ToClient) {
        if let Some(c) = self.panes.get(&pane).and_then(|p| p.owner) {
            self.send(c, m);
        }
    }

    /// The next queued command typed at a pane's prompt, once its line
    /// editor is reading (out of line mode), so it is not echoed twice.
    fn fire(&mut self, id: u64) {
        let Some(p) = self.panes.get_mut(&id) else { return };
        if !p.at_prompt || p.queue.paused || p.queue.items.is_empty() || !p.pty.raw() {
            return;
        }
        let cmd = p.queue.items.remove(0);
        let _ = p.keys.send(format!("\x15{cmd}\r").into_bytes());
        p.at_prompt = false;
        self.tell(id, ToClient::Ran { pane: id, cmd });
    }

    fn output(&mut self, id: u64, b: Vec<u8>) {
        let Some(p) = self.panes.get_mut(&id) else { return };
        p.log.push(&b);
        // Marks, a split one too.
        let seen = p.tail.len();
        let mut joined = std::mem::take(&mut p.tail);
        joined.extend_from_slice(&b);
        let found = marks(&joined, seen);
        p.tail = joined[joined.len().saturating_sub(15)..].to_vec();
        let mut ended = None;
        for m in found {
            match m {
                Mark::Prompt => p.at_prompt = true,
                Mark::Started => p.at_prompt = false,
                Mark::Ended(exit) => ended = Some(exit),
            }
        }
        // /tee: its part of the output to the file.
        let mut tee_end = None;
        if let Some(t) = &mut p.tee {
            let (part, done) = t.take(&b);
            let text = crate::app::automate::plain(part);
            t.state.lines += text.matches('\n').count() as u64;
            use std::io::Write as _;
            if let Err(e) = t.file.write_all(text.as_bytes()) {
                tee_end = Some(e.to_string());
            } else if done {
                tee_end = Some(String::new());
            }
        }
        if let Some(error) = tee_end {
            let lines = p.tee.take().map_or(0, |t| t.state.lines);
            self.tell(id, ToClient::TeeEnd { pane: id, lines, error });
        }
        // A failed command holds the queue behind it (as the window does,
        // but before the next is typed), unless it was being restarted.
        if let Some(exit) = ended {
            let failed = exit.is_some_and(|e| e != 0);
            if let Some(p) = self.panes.get_mut(&id) {
                if failed && !p.queue.items.is_empty() && !p.queue.restarting {
                    p.queue.paused = true;
                }
                p.queue.restarting = false;
            }
        }
        // Commands waiting on this one, in panes no window has, go on.
        if let Some(exit) = ended {
            let failed = exit.is_some_and(|e| e != 0);
            for (_, other) in self.panes.iter_mut().filter(|(_, o)| o.owner.is_none()) {
                let (go, stay): (Vec<_>, Vec<_>) = std::mem::take(&mut other.queue.after).into_iter().partition(|(t, _)| *t == id);
                other.queue.after = stay;
                if !go.is_empty() {
                    other.queue.items.extend(go.into_iter().map(|(_, c)| c));
                    if failed {
                        other.queue.paused = true;
                    }
                }
            }
        }
        self.tell(id, ToClient::Output { pane: id, bytes: b });
        self.fire(id);
    }

    /// A window come: the panes no window has, and the workspaces they
    /// were in, are its.
    fn welcome(&mut self, c: u64) {
        let mut held: Vec<Held> = self
            .panes
            .iter()
            .filter(|(_, p)| p.owner.is_none())
            .map(|(id, p)| Held { pane: *id, pid: p.pty.pid, cmd: p.cmd.clone(), log: p.log.encode(), tee: p.tee.as_ref().map(|t| t.state.clone()), queue: p.queue.clone() })
            .collect();
        held.sort_by_key(|h| h.pane);
        for p in self.panes.values_mut().filter(|p| p.owner.is_none()) {
            p.owner = Some(c);
            p.window = self.names.get(&c).copied().unwrap_or(0);
            p.raw = None;
            p.proc = None;
            if let Some(t) = &mut p.tee {
                t.told = t.state.lines;
            }
        }
        let left = std::mem::take(&mut self.left);
        let layout = merge(left.iter().map(|(_, l)| l.as_str())).and_then(|s| toml::to_string(&s).ok()).unwrap_or_default();
        self.layouts.insert(c, layout.clone());
        self.send(c, ToClient::Welcome { layout, panes: held });
    }

    /// A window come back after its connection dropped: the panes it had,
    /// taken from the connection it had if the session has not seen that
    /// drop yet. Its workspaces it has still; none are given.
    fn welcome_back(&mut self, c: u64, window: u64, want: &[u64]) {
        let stale: Vec<u64> = self.names.iter().filter(|(conn, w)| **w == window && **conn != c).map(|(conn, _)| *conn).collect();
        for old in stale {
            self.names.remove(&old);
            self.windows.retain(|w| *w != old);
            if let Some(l) = self.layouts.remove(&old) {
                self.layouts.insert(c, l);
            }
            if let Some(st) = self.conns.remove(&old) {
                let _ = st.shutdown(std::net::Shutdown::Both);
            }
        }
        if let Some(i) = self.left.iter().position(|(w, _)| *w == window) {
            let (_, l) = self.left.remove(i);
            self.layouts.insert(c, l);
        }
        let mut held = Vec::new();
        // Its own, and those it names that no other window has.
        let live: Vec<u64> = self.names.iter().filter(|(_, w)| **w != window).map(|(conn, _)| *conn).collect();
        for (id, p) in self.panes.iter_mut().filter(|(id, p)| p.window == window || (want.contains(id) && !p.owner.is_some_and(|o| live.contains(&o)))) {
            p.window = window;
            p.owner = Some(c);
            p.raw = None;
            p.proc = None;
            if let Some(t) = &mut p.tee {
                t.told = t.state.lines;
            }
            held.push(Held { pane: *id, pid: p.pty.pid, cmd: p.cmd.clone(), log: p.log.encode(), tee: p.tee.as_ref().map(|t| t.state.clone()), queue: p.queue.clone() });
        }
        held.sort_by_key(|h| h.pane);
        self.send(c, ToClient::Welcome { layout: String::new(), panes: held });
    }

    /// Its workspaces window `c` does not show: those of the other
    /// windows (said to be shown elsewhere), those of windows gone, and
    /// its panes in no workspace, as one.
    fn listing(&self, c: u64) -> ToClient {
        use crate::persist::{ids_in_order, Session, Spec, WorkspaceSpec};
        let mut workspaces = Vec::new();
        let mut named: Vec<u64> = Vec::new();
        let from_windows = self.layouts.iter().filter(|(w, _)| **w != c).map(|(_, l)| (l, true));
        let from_left = self.left.iter().map(|(_, l)| (l, false));
        for (text, elsewhere) in from_windows.chain(from_left) {
            let Ok(session) = toml::from_str::<Session>(text) else { continue };
            for ws in session.workspaces {
                let Some(layout) = &ws.layout else { continue };
                let ids: Vec<u64> = ids_in_order(layout).into_iter().filter(|id| self.panes.contains_key(id)).collect();
                // Gone, or this window's already.
                if ids.is_empty() || ids.iter().all(|id| self.panes.get(id).is_some_and(|p| p.owner == Some(c))) {
                    named.extend(&ids);
                    continue;
                }
                named.extend(&ids);
                // Shown elsewhere only if its panes are a live window's.
                let shown = elsewhere && ids.iter().any(|id| self.panes.get(id).is_some_and(|p| p.owner.is_some_and(|o| o != c)));
                workspaces.push(Listed { spec: toml::to_string(&ws).unwrap_or_default(), panes: ids, elsewhere: shown });
            }
        }
        let mut loose: Vec<u64> = self.panes.iter().filter(|(id, p)| p.owner.is_none() && !named.contains(id)).map(|(id, _)| *id).collect();
        loose.sort();
        if !loose.is_empty() {
            let panes: Vec<Spec> = loose.iter().map(|id| Spec::Pane { cmd: String::new(), cwd: None, pins: Vec::new(), empty: false, id: Some(*id), host: None }).collect();
            let layout = if panes.len() == 1 { panes.into_iter().next() } else { Some(Spec::Across { v: panes, sizes: Vec::new() }) };
            let ws = WorkspaceSpec { name: "Kept".into(), layout, ..Default::default() };
            workspaces.push(Listed { spec: toml::to_string(&ws).unwrap_or_default(), panes: loose, elsewhere: false });
        }
        ToClient::Listing { workspaces }
    }

    /// Panes window `c` takes to show: those no other window has. Their
    /// workspace, if a window gone left it, is the taker's now.
    fn take(&mut self, c: u64, panes: &[u64]) -> ToClient {
        let window = self.names.get(&c).copied().unwrap_or(0);
        let live: Vec<u64> = self.windows.iter().copied().filter(|w| *w != c).collect();
        let mut held = Vec::new();
        for id in panes {
            let Some(p) = self.panes.get_mut(id) else { continue };
            if p.owner.is_some_and(|o| live.contains(&o)) {
                continue;
            }
            p.owner = Some(c);
            p.window = window;
            p.raw = None;
            p.proc = None;
            if let Some(t) = &mut p.tee {
                t.told = t.state.lines;
            }
            held.push(Held { pane: *id, pid: p.pty.pid, cmd: p.cmd.clone(), log: p.log.encode(), tee: p.tee.as_ref().map(|t| t.state.clone()), queue: p.queue.clone() });
        }
        let taken: Vec<u64> = held.iter().map(|h| h.pane).collect();
        self.left.retain(|(_, l)| {
            toml::from_str::<crate::persist::Session>(l).map_or(true, |s| !s.workspaces.iter().any(|w| w.layout.as_ref().is_some_and(|l| crate::persist::ids_in_order(l).iter().any(|id| taken.contains(id)))))
        });
        ToClient::Taken { panes: held }
    }

    /// Every window's workspaces, and those left, as one session: saved,
    /// to be offered back after the machine restarts.
    fn save(&self) {
        if let Some(s) = merge(self.layouts.values().map(String::as_str).chain(self.left.iter().map(|(_, l)| l.as_str()))) {
            crate::persist::save_session(&s);
        }
    }

    fn info(&self) -> ToClient {
        let names = merge(self.layouts.values().map(String::as_str).chain(self.left.iter().map(|(_, l)| l.as_str()))).map(|s| s.workspaces.iter().map(|w| w.name.clone()).filter(|n| !n.is_empty()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        ToClient::Info { attached: !self.windows.is_empty(), panes: self.panes.len() as u32, names }
    }

    /// A new pane for window `c`: its name and its shell's process id (-1 if
    /// it could not start).
    fn spawn(&mut self, tx: &Sender<Ev>, c: u64, cmd: String, cwd: String, cols: u16, rows: u16, px: (u32, u32)) -> (u64, i32) {
        let pane = self.next_pane;
        self.next_pane += 1;
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
                self.panes.insert(pane, Pane { pty, keys, cmd, log, raw: None, tail: Vec::new(), at_prompt: false, queue: Queue::default(), tee: None, proc: None, owner: Some(c), window: self.names.get(&c).copied().unwrap_or(0) });
                pid
            }
            Err(e) => {
                eprintln!("jot serve: could not start a shell: {e}");
                -1
            }
        };
        (pane, pid)
    }

    fn tee(&mut self, pane: u64, state: TeeState) {
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let path = PathBuf::from(&state.path);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => p.tee = Some(Tee { state, file, told: 0 }),
            Err(e) => self.tell(pane, ToClient::TeeEnd { pane, lines: 0, error: e.to_string() }),
        }
    }
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
    let mut s = Server { panes: HashMap::new(), conns: HashMap::new(), windows: Vec::new(), layouts: HashMap::new(), names: HashMap::new(), left: Vec::new(), next_pane: 1 };
    let mut next_conn = 0u64;
    let started = Instant::now();
    while let Ok(ev) = rx.recv() {
        match ev {
            Ev::Conn(stream) => {
                next_conn += 1;
                let c = next_conn;
                let Ok(mut r) = stream.try_clone() else { continue };
                s.conns.insert(c, stream);
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
            Ev::Said(c, ToServer::Peek) => {
                let info = s.info();
                s.send(c, info);
                if let Some(st) = s.conns.remove(&c) {
                    let _ = st.shutdown(std::net::Shutdown::Both);
                }
            }
            Ev::Said(c, ToServer::Hello { proto, window, again, want }) => {
                if proto != PROTO {
                    s.send(c, ToClient::Refused { why: format!("protocol {PROTO} here, {proto} in the window") });
                    if let Some(st) = s.conns.remove(&c) {
                        let _ = st.shutdown(std::net::Shutdown::Both);
                    }
                    continue;
                }
                if !s.windows.contains(&c) {
                    s.windows.push(c);
                }
                s.names.insert(c, window);
                if again {
                    s.welcome_back(c, window, &want);
                } else {
                    s.welcome(c);
                }
            }
            Ev::Said(c, m) if s.windows.contains(&c) => match m {
                ToServer::Spawn { cmd, cwd, cols, rows, px } => {
                    let (pane, pid) = s.spawn(&tx, c, cmd, cwd, cols, rows, px);
                    s.send(c, ToClient::Spawned { pane, pid });
                }
                ToServer::Input { pane, bytes } => {
                    if let Some(p) = s.panes.get(&pane) {
                        let _ = p.keys.send(bytes);
                    }
                }
                ToServer::Resize { pane, cols, rows, px } => {
                    if let Some(p) = s.panes.get_mut(&pane) {
                        p.pty.resize(cols, rows, px);
                        p.log.size(cols, rows);
                    }
                }
                ToServer::Close { pane } => {
                    // Dropped: its shell hung up on.
                    s.panes.remove(&pane);
                }
                ToServer::Layout { text } => {
                    s.layouts.insert(c, text);
                    s.save();
                }
                ToServer::Tee { pane, tee } => s.tee(pane, tee),
                ToServer::TeeStop { pane } => {
                    if let Some(t) = s.panes.get_mut(&pane).and_then(|p| p.tee.take()) {
                        s.send(c, ToClient::TeeEnd { pane, lines: t.state.lines, error: String::new() });
                    }
                }
                ToServer::Queue { pane, queue } => {
                    if let Some(p) = s.panes.get_mut(&pane) {
                        p.queue = queue;
                    }
                    s.fire(pane);
                }
                ToServer::Signal { pane, sig, job } => {
                    if let Some(p) = s.panes.get(&pane) {
                        let who = if job { unsafe { libc::tcgetpgrp(p.pty.fd) } } else { p.pty.pid };
                        // Never the server's own group, nor every process.
                        if who > 1 {
                            unsafe { libc::kill(-who, sig) };
                        }
                    }
                }
                ToServer::List => {
                    let listing = s.listing(c);
                    s.send(c, listing);
                }
                ToServer::Take { panes } => {
                    let taken = s.take(c, &panes);
                    s.send(c, taken);
                }
                ToServer::Hello { .. } | ToServer::Peek => {}
            },
            Ev::Said(..) => {}
            Ev::Left(c) => {
                s.conns.remove(&c);
                if let Some(i) = s.windows.iter().position(|w| *w == c) {
                    s.windows.remove(i);
                    // Its panes and workspaces, for the next window; none if
                    // all its panes ended.
                    let mut any = false;
                    for p in s.panes.values_mut().filter(|p| p.owner == Some(c)) {
                        p.owner = None;
                        any = true;
                    }
                    let name = s.names.remove(&c).unwrap_or(0);
                    if let Some(l) = s.layouts.remove(&c).filter(|_| any) {
                        s.left.push((name, l));
                    }
                    s.save();
                    if s.panes.is_empty() && s.windows.is_empty() {
                        break;
                    }
                }
            }
            Ev::Out(id, b) => s.output(id, b),
            Ev::Ended(id) => {
                let owner = s.panes.get(&id).and_then(|p| p.owner);
                if s.panes.remove(&id).is_some() {
                    if let Some(c) = owner {
                        s.send(c, ToClient::Exit { pane: id });
                    }
                }
                crate::pty::reap();
                if s.panes.is_empty() && s.windows.is_empty() {
                    break;
                }
            }
            Ev::Tick => {
                crate::pty::reap();
                let ids: Vec<u64> = s.panes.keys().copied().collect();
                for id in ids {
                    s.fire(id);
                    let Some(p) = s.panes.get_mut(&id) else { continue };
                    if p.owner.is_none() {
                        continue;
                    }
                    let raw = p.pty.raw();
                    let mode = (p.raw != Some(raw)).then(|| {
                        p.raw = Some(raw);
                        ToClient::Mode { pane: id, raw }
                    });
                    let now = p.pty.proc();
                    let proc = (p.proc.as_ref() != Some(&now)).then(|| {
                        p.proc = Some(now.clone());
                        ToClient::Proc { pane: id, proc: now }
                    });
                    let lines = p.tee.as_mut().filter(|t| t.told != t.state.lines).map(|t| {
                        t.told = t.state.lines;
                        ToClient::TeeLines { pane: id, lines: t.state.lines }
                    });
                    for m in [mode, proc, lines].into_iter().flatten() {
                        s.tell(id, m);
                    }
                }
                // Started but never taken up: nothing to keep it.
                if s.windows.is_empty() && s.panes.is_empty() && started.elapsed() > Duration::from_secs(30) {
                    break;
                }
            }
        }
    }
    let _ = std::fs::remove_file(path);
}

/// Sessions' workspaces, one after another: several windows' layouts as
/// one. None when there are none.
fn merge<'a>(texts: impl Iterator<Item = &'a str>) -> Option<crate::persist::Session> {
    let mut out: Option<crate::persist::Session> = None;
    for s in texts.filter_map(|t| toml::from_str::<crate::persist::Session>(t).ok()) {
        match &mut out {
            None => out = Some(s),
            Some(o) => {
                o.workspaces.extend(s.workspaces);
                o.popouts.extend(s.popouts);
                if o.roam.is_none() {
                    o.roam = s.roam;
                }
            }
        }
    }
    out
}
