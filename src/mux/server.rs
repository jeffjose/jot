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
    /// The windows showing it, by their connections: any number, each a
    /// view of the same terminal.
    viewers: Vec<u64>,
    /// The names of the windows that show it, or last did.
    windows: Vec<u64>,
    /// The size each viewer gives it, and the size it has: the one typed
    /// into last.
    sizes: HashMap<u64, (u16, u16, (u32, u32))>,
    size: (u16, u16, (u32, u32)),
    /// Its number, the same in every window (ToClient::Numbers): the
    /// lowest free when it started, unless renumbered.
    number: u32,
}

impl Pane {
    /// Shown in window `c` (named `name`) too: told again what it is
    /// doing.
    fn add_viewer(&mut self, c: u64, name: u64) {
        if !self.viewers.contains(&c) {
            self.viewers.push(c);
        }
        if !self.windows.contains(&name) {
            self.windows.push(name);
        }
        self.raw = None;
        self.proc = None;
        if let Some(t) = &mut self.tee {
            t.told = t.state.lines;
        }
    }

    fn held(&self, id: u64) -> Held {
        Held { pane: id, pid: self.pty.pid, cmd: self.cmd.clone(), log: self.log.encode(), tee: self.tee.as_ref().map(|t| t.state.clone()), queue: self.queue.clone() }
    }

    /// The size viewer `c` gives it, made its own if it is not already.
    /// Whether that changed it: every window showing it is then told, so
    /// each draws it at the one size its program writes for.
    fn size_for(&mut self, c: u64) -> bool {
        let Some(s) = self.sizes.get(&c).copied().filter(|s| *s != self.size) else { return false };
        crate::debuglog::log(format!("session: pane {}: {}x{} -> {}x{}, window {c}'s", self.pty.pid, self.size.0, self.size.1, s.0, s.1));
        self.pty.resize(s.0, s.1, s.2);
        self.log.size(s.0, s.1);
        self.size = s;
        true
    }

    /// Window `c` no longer shows it: if its size was the one, another
    /// showing it gives the size. Whether that changed it.
    fn drop_viewer(&mut self, c: u64) -> bool {
        self.viewers.retain(|v| *v != c);
        self.sizes.remove(&c);
        if self.sizes.values().any(|s| *s == self.size) {
            return false;
        }
        match self.viewers.last().copied() {
            Some(other) => self.size_for(other),
            None => false,
        }
    }

    fn size_msg(&self, pane: u64) -> ToClient {
        ToClient::Size { pane, cols: self.size.0, rows: self.size.1 }
    }
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
    /// The machine's desktop, as a window here last had it (or found on
    /// it), for panes asked for from no desktop of this machine's.
    desktop: Option<Env>,
    /// What each window said it is, for jot status.
    abouts: HashMap<u64, String>,
    /// The workspaces the session shares among the windows that can share
    /// them (Hello's caps), each with its id and rev, in their order.
    shared: Vec<crate::persist::WorkspaceSpec>,
    /// The windows that share them; and for each, the shared ones its last
    /// layout had.
    sharing: Vec<u64>,
    seen: HashMap<u64, Vec<u64>>,
    /// Which window changed each shared workspace last: its changes after
    /// it are taken, though it has not been told its own yet.
    changed_by: HashMap<u64, u64>,
    /// The windows told terminals' numbers (Hello's caps).
    numbering: Vec<u64>,
}

impl Server {
    fn send(&mut self, c: u64, m: ToClient) {
        if let Some(s) = self.conns.get_mut(&c) {
            if s.write_all(&m.encode()).is_err() {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
        }
    }

    /// Window `c`, given panes, told the size each is now: a window may
    /// show it at another size than its own until typed into.
    fn tell_sizes(&mut self, c: u64, panes: &[u64]) {
        let sizes: Vec<ToClient> = panes.iter().filter_map(|id| self.panes.get(id).map(|p| p.size_msg(*id))).collect();
        for m in sizes {
            self.send(c, m);
        }
    }

    /// Said to every window showing a pane.
    fn tell(&mut self, pane: u64, m: ToClient) {
        let viewers = self.panes.get(&pane).map(|p| p.viewers.clone()).unwrap_or_default();
        if viewers.is_empty() {
            return;
        }
        let frame = m.encode();
        for c in viewers {
            if let Some(s) = self.conns.get_mut(&c) {
                if s.write_all(&frame).is_err() {
                    let _ = s.shutdown(std::net::Shutdown::Both);
                }
            }
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
            for (_, other) in self.panes.iter_mut().filter(|(_, o)| o.viewers.is_empty()) {
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

    /// A window come: the workspaces of one window that quit, and their
    /// panes, are its, with the panes no window has that no other window
    /// that quit had. The other windows' wait, each for a window of its
    /// own: how many is said with the layout (`more`), and the window
    /// opens them.
    fn welcome(&mut self, c: u64) {
        let (quake, mut left): (Vec<_>, Vec<_>) = std::mem::take(&mut self.left).into_iter().partition(|(w, _)| *w == super::QUAKE_WINDOW);
        let mine = (!left.is_empty()).then(|| left.remove(0));
        // A window that shares workspaces has every one the session shares,
        // and their terminals, whatever other windows show them: they wait
        // for no other window.
        let sharing = self.sharing.contains(&c);
        let shared_panes: Vec<u64> = if sharing { self.shared_panes() } else { Vec::new() };
        let waiting: Vec<u64> = left.iter().flat_map(|(_, l)| layout_ids(l)).filter(|id| !shared_panes.contains(id)).collect();
        self.left = quake;
        self.left.extend(left);
        let more = self.left.iter().filter(|(w, _)| *w != super::QUAKE_WINDOW).count() as u32;
        // The quake window's panes and workspaces wait for the next one, and
        // the other windows' for theirs.
        let free = |id: &u64, p: &Pane| shared_panes.contains(id) || p.viewers.is_empty() && !p.windows.contains(&super::QUAKE_WINDOW) && !waiting.contains(id);
        let mut held: Vec<Held> = self.panes.iter().filter(|(id, p)| free(id, p)).map(|(id, p)| p.held(*id)).collect();
        held.sort_by_key(|h| h.pane);
        let name = self.names.get(&c).copied().unwrap_or(0);
        for (_, p) in self.panes.iter_mut().filter(|(id, p)| free(id, p)) {
            p.add_viewer(c, name);
        }
        let mine = mine.and_then(|(_, l)| toml::from_str::<crate::persist::Session>(&l).ok());
        let layout = if sharing && (mine.is_some() || !self.shared.is_empty()) {
            // The shared ones, in their order, then any of its own the session
            // does not share (yet: shared once the window says them); shown,
            // the one it showed.
            let mut s = mine.unwrap_or_default();
            let showing = s.current_id.or_else(|| s.workspaces.get(s.current).and_then(|w| w.id));
            let ids: Vec<u64> = self.shared.iter().filter_map(|w| w.id).collect();
            let own: Vec<_> = std::mem::take(&mut s.workspaces).into_iter().filter(|w| !w.id.is_some_and(|id| ids.contains(&id))).collect();
            s.workspaces = self.shared.clone();
            s.workspaces.extend(own);
            s.current = showing.and_then(|id| s.workspaces.iter().position(|w| w.id == Some(id))).unwrap_or(0);
            s.current_id = showing.filter(|id| ids.contains(id));
            s.more = more;
            self.seen.insert(c, ids);
            toml::to_string(&s).unwrap_or_default()
        } else {
            mine.map(|mut s| {
                s.more = more;
                s
            })
            .and_then(|s| toml::to_string(&s).ok())
            .unwrap_or_default()
        };
        self.layouts.insert(c, layout.clone());
        let ids: Vec<u64> = held.iter().map(|h| h.pane).collect();
        self.send(c, ToClient::Welcome { layout, panes: held });
        self.tell_sizes(c, &ids);
    }

    /// A window come back after its connection dropped: the panes it had,
    /// taken from the connection it had if the session has not seen that
    /// drop yet. Its workspaces it has still; none are given.
    fn welcome_back(&mut self, c: u64, window: u64, want: &[u64]) {
        let stale: Vec<u64> = self.names.iter().filter(|(conn, w)| **w == window && **conn != c).map(|(conn, _)| *conn).collect();
        for old in stale {
            self.names.remove(&old);
            self.sharing.retain(|w| *w != old);
            self.numbering.retain(|w| *w != old);
            self.seen.remove(&old);
            self.windows.retain(|w| *w != old);
            if let Some(l) = self.layouts.remove(&old) {
                self.layouts.insert(c, l);
            }
            if let Some(st) = self.conns.remove(&old) {
                let _ = st.shutdown(std::net::Shutdown::Both);
            }
            for p in self.panes.values_mut() {
                p.viewers.retain(|v| *v != old);
            }
        }
        if let Some(i) = self.left.iter().position(|(w, _)| *w == window) {
            let (_, l) = self.left.remove(i);
            self.layouts.insert(c, l);
        }
        let mut held = Vec::new();
        // Its own, and those it names, whatever other windows show them.
        for (id, p) in self.panes.iter_mut().filter(|(id, p)| p.windows.contains(&window) || want.contains(id)) {
            p.add_viewer(c, window);
            // Told again on the next tick, to the new window as well: what
            // runs in front, its folder, how it reads keys (told only when
            // they change, so it would wait for the next change).
            p.proc = None;
            p.raw = None;
            held.push(p.held(*id));
        }
        held.sort_by_key(|h| h.pane);
        // Its workspaces, as it last said them: a window that kept them
        // (reconnecting) passes them by; one started again (the quake window)
        // takes them up.
        let layout = self.layouts.get(&c).cloned().unwrap_or_default();
        let ids: Vec<u64> = held.iter().map(|h| h.pane).collect();
        self.send(c, ToClient::Welcome { layout, panes: held });
        self.tell_sizes(c, &ids);
        // Sharing: the shared workspaces as they are now, changed meanwhile
        // perhaps (or the session's newer); the ones its layout had are
        // what it has seen.
        if self.sharing.contains(&c) {
            let had = self.layouts.get(&c).and_then(|l| toml::from_str::<crate::persist::Session>(l).ok()).map(|s| s.workspaces.iter().filter_map(|w| w.id).collect::<Vec<_>>()).unwrap_or_default();
            let ids: Vec<u64> = self.shared.iter().filter_map(|w| w.id).filter(|id| had.contains(id)).collect();
            self.seen.insert(c, ids);
            self.share_to(c);
        }
    }

    /// The terminals of the workspaces the session shares, those it has.
    fn shared_panes(&self) -> Vec<u64> {
        self.shared.iter().filter_map(|w| w.layout.as_ref()).flat_map(crate::persist::ids_in_order).filter(|id| self.panes.contains_key(id)).collect()
    }

    /// The shared workspaces told to window `c`, with the terminals in them
    /// it does not have yet, now its.
    fn share_to(&mut self, c: u64) {
        let specs: Vec<String> = self.shared.iter().filter_map(|w| toml::to_string(w).ok()).collect();
        let name = self.names.get(&c).copied().unwrap_or(0);
        let mut held = Vec::new();
        for id in self.shared_panes() {
            if let Some(p) = self.panes.get_mut(&id).filter(|p| !p.viewers.contains(&c)) {
                p.add_viewer(c, name);
                held.push(p.held(id));
            }
        }
        let ids: Vec<u64> = held.iter().map(|h| h.pane).collect();
        self.send(c, ToClient::Workspaces { specs, held });
        self.tell_sizes(c, &ids);
    }

    /// Window `c`'s workspaces, as it says them, taken into those the
    /// session shares: one it made, added after the one before it; one it
    /// changed (its layout's shape, its name or colors), changed, unless
    /// changed since by another (it had not seen that yet, and it is its
    /// echo); one it had and has no more, closed. Sizes, folders and focus
    /// are each window's own, and change nothing shared. Every window
    /// that shares them is told, when they change.
    fn take_shared(&mut self, c: u64, text: &str) {
        let Ok(session) = toml::from_str::<crate::persist::Session>(text) else { return };
        let before = self.seen.get(&c).cloned().unwrap_or_default();
        let mut now = Vec::new();
        let mut changed = false;
        let mut after: Option<u64> = None;
        for ws in session.workspaces.iter().filter(|w| w.shareable()) {
            let Some(id) = ws.id else { continue };
            now.push(id);
            let rev = ws.rev.unwrap_or(0);
            match self.shared.iter().position(|s| s.id == Some(id)) {
                Some(i) => {
                    let current = self.shared[i].rev.unwrap_or(0);
                    if rev < current && self.changed_by.get(&id) != Some(&c) {
                        crate::debuglog::log(format!("session: window {c}: {} rev {rev}, older than the list's {current}: not taken", ws.name));
                    }
                    if rev >= current || self.changed_by.get(&id) == Some(&c) {
                        let moved = ws.shared_shape() != self.shared[i].shared_shape();
                        if moved {
                            crate::debuglog::log(format!("session: window {c}: {} rev {rev} (the list's {current}), changed: taken", ws.name));
                        }
                        let mut w = ws.clone();
                        w.rev = Some(if moved { current + 1 } else { current });
                        self.shared[i] = w;
                        if moved {
                            self.changed_by.insert(id, c);
                        }
                        changed |= moved;
                    }
                }
                // New, made in this window.
                None if !before.contains(&id) => {
                    let mut w = ws.clone();
                    w.rev = Some(1);
                    let at = after.and_then(|a| self.shared.iter().position(|s| s.id == Some(a))).map_or(self.shared.len(), |i| i + 1);
                    self.shared.insert(at, w);
                    crate::debuglog::log(format!("session: window {c}: {} new, shared", ws.name));
                    self.changed_by.insert(id, c);
                    changed = true;
                }
                // Closed in another window since it said it last.
                None => {}
            }
            after = Some(id);
        }
        // Closed in this window.
        for id in before.iter().filter(|id| !now.contains(id)) {
            if let Some(i) = self.shared.iter().position(|s| s.id == Some(*id)) {
                crate::debuglog::log(format!("session: window {c}: {} gone from its layout: closed", self.shared[i].name));
                self.shared.remove(i);
                changed = true;
            }
        }
        self.seen.insert(c, now);
        if changed {
            crate::debuglog::log(format!("session: shared workspaces changed by window {c}: {}", self.shared.len()));
            for w in self.sharing.clone() {
                self.share_to(w);
            }
        }
    }

    /// Its workspaces window `c` does not show: those of the other
    /// windows (said to be shown elsewhere), those of windows gone, and
    /// its panes in no workspace, as one.
    fn listing(&self, c: u64) -> ToClient {
        use crate::persist::{ids_in_order, Session, Spec, WorkspaceSpec};
        let mut workspaces = Vec::new();
        let mut named: Vec<u64> = Vec::new();
        // The quake window's workspace is its own: another window is never
        // offered it to open, open or not (its terminals are listed, and any
        // can be taken into a workspace of another's).
        let quake = |w: u64| w == super::QUAKE_WINDOW;
        let asker_quake = self.names.get(&c).is_some_and(|w| quake(*w));
        let asker_shares = self.sharing.contains(&c);
        let from_windows = self.layouts.iter().filter(|(w, _)| **w != c).map(|(w, l)| (l, true, self.names.get(w).is_some_and(|n| quake(*n))));
        let from_left = self.left.iter().map(|(name, l)| (l, false, quake(*name)));
        for (text, elsewhere, theirs) in from_windows.chain(from_left) {
            let kept = theirs && !asker_quake;
            let Ok(session) = toml::from_str::<Session>(text) else { continue };
            for ws in session.workspaces {
                let Some(layout) = &ws.layout else { continue };
                let ids: Vec<u64> = ids_in_order(layout).into_iter().filter(|id| self.panes.contains_key(id)).collect();
                // Gone, or this window's already (a shared one, in its bar).
                let shared = asker_shares && ws.id.is_some_and(|id| self.shared.iter().any(|s| s.id == Some(id)));
                if shared || ids.is_empty() || ids.iter().all(|id| self.panes.get(id).is_some_and(|p| p.viewers.contains(&c))) {
                    named.extend(&ids);
                    continue;
                }
                named.extend(&ids);
                // Shown elsewhere only if its panes are a live window's.
                let shown = kept || elsewhere && ids.iter().any(|id| self.panes.get(id).is_some_and(|p| p.viewers.iter().any(|v| *v != c)));
                workspaces.push(Listed { spec: toml::to_string(&ws).unwrap_or_default(), panes: ids, elsewhere: shown, label: String::new(), running: false });
            }
        }
        let mut loose: Vec<u64> = self.panes.iter().filter(|(id, p)| p.viewers.is_empty() && !named.contains(id)).map(|(id, _)| *id).collect();
        loose.sort();
        // Panes in no workspace, each on its own: what it runs, or where.
        for id in loose {
            let Some(p) = self.panes.get(&id) else { continue };
            let proc = p.pty.proc();
            let label = if proc.busy && !proc.fg.is_empty() { proc.fg.clone() } else { crate::persist::shorten(std::path::Path::new(&proc.cwd)) };
            let spec = Spec::Pane { cmd: String::new(), cwd: None, pins: Vec::new(), empty: false, id: Some(id), host: None };
            let ws = WorkspaceSpec { layout: Some(spec), ..Default::default() };
            workspaces.push(Listed { spec: toml::to_string(&ws).unwrap_or_default(), panes: vec![id], elsewhere: false, label, running: proc.busy });
        }
        // What each terminal runs now, and where: one another window shows
        // named as it is.
        let procs = self.panes.iter().map(|(id, p)| {
            let proc = p.pty.proc();
            (*id, proc.fg.clone(), proc.busy, crate::persist::shorten(std::path::Path::new(&proc.cwd)))
        }).collect();
        ToClient::Listing { workspaces, procs }
    }

    /// Panes window `c` takes to show: those no other window has. Their
    /// workspace, if a window gone left it, is the taker's now.
    fn take(&mut self, c: u64, panes: &[u64]) -> ToClient {
        let window = self.names.get(&c).copied().unwrap_or(0);
        let mut held = Vec::new();
        // Shown here too, whatever other windows show them.
        for id in panes {
            let Some(p) = self.panes.get_mut(id) else { continue };
            p.add_viewer(c, window);
            // Told again on the next tick, to the new window as well: what
            // runs in front, its folder, how it reads keys (told only when
            // they change, so it would wait for the next change).
            p.proc = None;
            p.raw = None;
            held.push(p.held(*id));
        }
        // Shared terminals are in every sharing window's layout, and take
        // no window gone's workspace with them.
        let shared = self.shared_panes();
        let taken: Vec<u64> = held.iter().map(|h| h.pane).filter(|id| !shared.contains(id)).collect();
        self.left.retain(|(_, l)| {
            toml::from_str::<crate::persist::Session>(l).map_or(true, |s| !s.workspaces.iter().any(|w| w.layout.as_ref().is_some_and(|l| crate::persist::ids_in_order(l).iter().any(|id| taken.contains(id)))))
        });
        ToClient::Taken { panes: held }
    }

    /// Every window's workspaces, and those left, as one session, and each
    /// window's on its own as well: saved, to be offered back after the
    /// machine restarts, each window as it was. The quake window's go with
    /// the first window's (it is placed by its settings).
    fn save(&self) {
        if let Some(s) = self.saved() {
            crate::persist::save_session(&s);
        }
    }

    /// The session as saved (see save).
    fn saved(&self) -> Option<crate::persist::Session> {
        use crate::persist::Session;
        // The shared workspaces once, from the session's list; each window's
        // layout less them, what it showed of them by its id.
        let ids: Vec<u64> = self.shared.iter().filter_map(|w| w.id).collect();
        let own = |text: &str| -> Option<Session> {
            let mut s = toml::from_str::<Session>(text).ok()?;
            let showing = s.workspaces.get(s.current).and_then(|w| w.id).filter(|id| ids.contains(id));
            let current = s.current;
            let mut kept = 0;
            let mut at = 0;
            s.workspaces = std::mem::take(&mut s.workspaces)
                .into_iter()
                .enumerate()
                .filter(|(i, w)| {
                    let keep = !w.id.is_some_and(|id| ids.contains(&id));
                    if keep {
                        if *i == current {
                            at = kept;
                        }
                        kept += 1;
                    }
                    keep
                })
                .map(|(_, w)| w)
                .collect();
            s.current = at;
            s.current_id = showing.or(s.current_id.filter(|id| ids.contains(id)));
            Some(s)
        };
        // With the shared ones before its own, as a window that restores
        // them all has them.
        let with_shared = |mut s: Session| -> Session {
            let n = self.shared.len();
            s.current = s.current_id.and_then(|id| ids.iter().position(|i| *i == id)).unwrap_or(s.current + n);
            let mut all = self.shared.clone();
            all.extend(std::mem::take(&mut s.workspaces));
            s.workspaces = all;
            s
        };
        let open = self.layouts.iter().map(|(c, l)| (self.names.get(c).copied().unwrap_or(0), l));
        let all: Vec<(u64, Session)> = open.chain(self.left.iter().map(|(w, l)| (*w, l))).filter_map(|(w, l)| own(l).map(|s| (w, s))).collect();
        if all.is_empty() && self.shared.is_empty() {
            return None;
        }
        // Every window's together (all an older jot reads): the first's, the
        // others' workspaces and pop-outs after.
        let mut s = all.first().map(|(_, s)| s.clone()).unwrap_or_default();
        for (_, o) in all.iter().skip(1) {
            s.workspaces.extend(o.workspaces.iter().cloned());
            s.popouts.extend(o.popouts.iter().cloned());
            if s.roam.is_none() {
                s.roam = o.roam.clone();
            }
        }
        let mut s = with_shared(s);
        // And each window on its own, to be opened as it was: the first with
        // the shared ones, which every other gets from the session.
        let mut groups: Vec<(u64, Session)> = all.iter().filter(|(w, _)| *w != super::QUAKE_WINDOW).filter(|(_, s)| !s.workspaces.is_empty() || !s.popouts.is_empty() || s.current_id.is_some() || s.window.is_some()).cloned().collect();
        groups.sort_by_key(|(w, _)| *w);
        if groups.len() > 1 {
            let first = groups.remove(0).1;
            let mut first = with_shared(first);
            for (_, q) in all.iter().filter(|(w, _)| *w == super::QUAKE_WINDOW) {
                first.workspaces.extend(q.workspaces.iter().cloned());
            }
            s.windows = std::iter::once(first).chain(groups.into_iter().map(|(_, s)| s)).collect();
        }
        Some(s)
    }

    fn info(&self) -> ToClient {
        let names = merge(self.layouts.values().map(String::as_str).chain(self.left.iter().map(|(_, l)| l.as_str()))).map(|s| s.workspaces.iter().map(|w| w.name.clone()).filter(|n| !n.is_empty()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        let list = self
            .windows
            .iter()
            .map(|c| {
                // The workspace it shows now.
                let names = self.layouts.get(c).and_then(|l| toml::from_str::<crate::persist::Session>(l).ok()).and_then(|s| s.workspaces.get(s.current.min(s.workspaces.len().saturating_sub(1))).map(|w| w.name.clone())).unwrap_or_default();
                let panes = self.panes.values().filter(|p| p.viewers.contains(c)).count() as u32;
                (self.abouts.get(c).cloned().unwrap_or_default(), names, panes)
            })
            .collect();
        let free = self.panes.values().filter(|p| p.viewers.is_empty()).count() as u32;
        ToClient::Info { attached: !self.windows.is_empty(), panes: self.panes.len() as u32, names, version: crate::version(), windows: self.windows.len() as u32, free, list, detail: self.detail() }
    }

    /// The desktop window `c`'s new pane is given: the window's own,
    /// when it is on one; else this machine's, when it has one (a window
    /// on another machine, through `jot bridge`, has none of its own).
    fn desktop(&mut self, c: u64) -> Env {
        match self.conns.get(&c).and_then(desktop_of) {
            Some((false, env)) => {
                self.desktop = Some(env.clone());
                return env;
            }
            // Over `ssh -X`: its own, but not the machine's.
            Some((true, env)) => return env,
            None => {}
        }
        if self.desktop.is_none() {
            self.desktop = find_desktop();
        }
        self.desktop.clone().unwrap_or_default()
    }

    /// A new pane for window `c`: its name and its shell's process id (-1 if
    /// it could not start).
    fn spawn(&mut self, tx: &Sender<Ev>, c: u64, cmd: String, cwd: String, cols: u16, rows: u16, px: (u32, u32)) -> (u64, i32) {
        let pane = self.next_pane;
        self.next_pane += 1;
        let desktop = self.desktop(c);
        let number = self.free_number();
        let pid = match Local::spawn(&cmd, Path::new(&cwd), cols, rows, px, &desktop) {
            Ok(pty) => {
                let pid = pty.pid;
                let keys = start_io(tx, pane, &pty);
                let mut log = Log::default();
                log.size(cols, rows);
                self.panes.insert(pane, Pane { pty, keys, cmd, log, raw: None, tail: Vec::new(), at_prompt: false, queue: Queue::default(), tee: None, proc: None, viewers: vec![c], windows: vec![self.names.get(&c).copied().unwrap_or(0)], sizes: HashMap::from([(c, (cols, rows, px))]), size: (cols, rows, px), number });
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

/// `jot serve PATH`: the session, until its last pane ends. With a
/// handover file, the session the server before this one (in this same
/// process) handed over: its panes and socket taken up as they were.
pub fn run(path: &Path, handover: Option<&Path>) {
    use std::os::fd::{AsRawFd, FromRawFd};
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }
    let (tx, rx) = channel::<Ev>();
    let (mut s, listener) = match handover.map(|f| resume(f, &tx)) {
        Some(Ok((s, fd))) => (s, unsafe { UnixListener::from_raw_fd(fd) }),
        Some(Err(e)) => {
            eprintln!("jot serve: could not take up the session handed over: {e}");
            return;
        }
        None => match UnixListener::bind(path) {
            Ok(l) => (Server { panes: HashMap::new(), conns: HashMap::new(), windows: Vec::new(), layouts: HashMap::new(), names: HashMap::new(), left: Vec::new(), next_pane: 1, desktop: None, abouts: HashMap::new(), shared: Vec::new(), sharing: Vec::new(), seen: HashMap::new(), changed_by: HashMap::new(), numbering: Vec::new() }, l),
            Err(e) => {
                eprintln!("jot serve: {}: {e}", path.display());
                return;
            }
        },
    };
    let listener_fd = listener.as_raw_fd();
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
            // `jot restart`: what the panes printed meanwhile passed on
            // first, then the server swapped for the jot installed now.
            Ev::Said(c, ToServer::Restart { exe }) => {
                while let Ok(ev) = rx.recv_timeout(Duration::from_millis(30)) {
                    match ev {
                        Ev::Out(id, b) => s.output(id, b),
                        Ev::Ended(id) => {
                            s.tell(id, ToClient::Exit { pane: id });
                            s.panes.remove(&id);
                        }
                        // A window that connected meanwhile reaches the new
                        // server instead, as it tries again.
                        _ => {}
                    }
                }
                crate::pty::reap();
                s.send(c, ToClient::Restarting);
                let e = s.handover(&exe, listener_fd, path);
                eprintln!("jot serve: could not restart as {exe}: {e}");
            }
            Ev::Said(c, ToServer::Hello { proto, window, again, want, about, caps }) => {
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
                s.abouts.insert(c, about);
                if caps & super::CAN_NUMBER != 0 && !s.numbering.contains(&c) {
                    s.numbering.push(c);
                }
                if caps & super::CAN_SHARE != 0 && !s.sharing.contains(&c) {
                    s.sharing.push(c);
                }
                if again {
                    s.welcome_back(c, window, &want);
                } else {
                    s.welcome(c);
                }
                s.tell_numbers(Some(c));
            }
            Ev::Said(c, m) if s.windows.contains(&c) => match m {
                ToServer::Spawn { cmd, cwd, cols, rows, px } => {
                    let (pane, pid) = s.spawn(&tx, c, cmd, cwd, cols, rows, px);
                    s.send(c, ToClient::Spawned { pane, pid });
                    s.tell_numbers(None);
                }
                // Typed into from a window: the size that window gives it, first.
                ToServer::Input { pane, bytes } => {
                    if let Some(p) = s.panes.get_mut(&pane) {
                        let resized = p.size_for(c);
                        let _ = p.keys.send(bytes);
                        if resized {
                            let m = p.size_msg(pane);
                            s.tell(pane, m);
                        }
                    }
                }
                ToServer::Resize { pane, cols, rows, px } => {
                    if let Some(p) = s.panes.get_mut(&pane) {
                        p.sizes.insert(c, (cols, rows, px));
                        if p.size_for(c) {
                            let m = p.size_msg(pane);
                            s.tell(pane, m);
                        }
                    }
                }
                ToServer::Close { pane } => {
                    // Dropped: its shell hung up on.
                    s.panes.remove(&pane);
                    s.tell_numbers(None);
                }
                ToServer::Layout { text } => {
                    if s.sharing.contains(&c) {
                        s.take_shared(c, &text);
                    }
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
                    s.tell_sizes(c, &panes);
                }
                ToServer::Release { pane } => {
                    // Let go: no longer one of the window's, to be given back
                    // when it reaches the session again (only what it shows
                    // is).
                    let name = s.names.get(&c).copied();
                    if let Some(p) = s.panes.get_mut(&pane) {
                        p.windows.retain(|w| Some(*w) != name);
                        if p.drop_viewer(c) {
                            let m = p.size_msg(pane);
                            s.tell(pane, m);
                        }
                    }
                }
                // A window's renumbering (a row moved in its sidebar): taken,
                // and told to every window.
                ToServer::Renumber { numbers } => {
                    s.renumber(&numbers);
                    s.tell_numbers(None);
                }
                ToServer::Hello { .. } | ToServer::Peek | ToServer::Restart { .. } => {}
            },
            Ev::Said(..) => {}
            Ev::Left(c) => {
                s.conns.remove(&c);
                if let Some(i) = s.windows.iter().position(|w| *w == c) {
                    s.windows.remove(i);
                    // Its panes and workspaces, for the next window; none if
                    // all its panes ended.
                    let mut any = false;
                    let mut resized = Vec::new();
                    for (id, p) in s.panes.iter_mut().filter(|(_, p)| p.viewers.contains(&c)) {
                        if p.drop_viewer(c) {
                            resized.push(p.size_msg(*id));
                        }
                        any = true;
                    }
                    for m in resized {
                        if let ToClient::Size { pane, .. } = m {
                            s.tell(pane, m);
                        }
                    }
                    let name = s.names.remove(&c).unwrap_or(0);
                    s.abouts.remove(&c);
                    s.sharing.retain(|w| *w != c);
                    s.numbering.retain(|w| *w != c);
                    s.seen.remove(&c);
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
                s.tell(id, ToClient::Exit { pane: id });
                s.panes.remove(&id);
                s.tell_numbers(None);
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
                    if p.viewers.is_empty() {
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

/// What a pane's programs need of the desktop: the display, the session's
/// bus and what kind it is. The session has them as whatever started it
/// had them, and that may have been an SSH login (`jot bridge`, a
/// `jot restart` over SSH), with none: a pane is given the machine's own.
const DESKTOP: [&str; 8] = ["DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY", "XDG_SESSION_TYPE", "XDG_CURRENT_DESKTOP", "XDG_SESSION_DESKTOP", "DESKTOP_SESSION", "DBUS_SESSION_BUS_ADDRESS"];

type Env = Vec<(std::ffi::CString, Option<std::ffi::CString>)>;

/// A desktop, as a process's environment has it: whether it came over
/// SSH (`ssh -X`), and the names above, set or not. None off a desktop.
fn desktop_in(raw: &[u8]) -> Option<(bool, Env)> {
    let vars: HashMap<&[u8], &[u8]> = raw.split(|b| *b == 0).filter_map(|kv| kv.iter().position(|b| *b == b'=').map(|i| (&kv[..i], &kv[i + 1..]))).collect();
    let on = |k: &str| vars.get(k.as_bytes()).is_some_and(|v| !v.is_empty());
    if !on("DISPLAY") && !on("WAYLAND_DISPLAY") {
        return None;
    }
    let env = DESKTOP.iter().filter_map(|k| Some((std::ffi::CString::new(*k).ok()?, vars.get(k.as_bytes()).and_then(|v| std::ffi::CString::new(*v).ok())))).collect();
    Some((on("SSH_CONNECTION"), env))
}

/// The desktop of the process at the other end of `s`, if it is on one:
/// a window here, not `jot bridge` (a window on another machine).
fn desktop_of(s: &UnixStream) -> Option<(bool, Env)> {
    use std::os::fd::AsRawFd;
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let got = unsafe { libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut libc::c_void, &mut len) };
    if got != 0 || cred.pid <= 0 {
        return None;
    }
    desktop_in(&std::fs::read(format!("/proc/{}/environ", cred.pid)).ok()?)
}

/// The machine's desktop, as one of this user's processes on it has it
/// (not one over SSH): the oldest, the desktop's session itself, sooner
/// than something started from it since.
fn find_desktop() -> Option<Env> {
    let uid = unsafe { libc::getuid() };
    let mut pids: Vec<u32> = std::fs::read_dir("/proc").ok()?.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect();
    pids.sort_unstable();
    pids.into_iter().find_map(|pid| {
        use std::os::unix::fs::MetadataExt;
        if std::fs::metadata(format!("/proc/{pid}")).ok()?.uid() != uid {
            return None;
        }
        match desktop_in(&std::fs::read(format!("/proc/{pid}/environ")).ok()?)? {
            (false, env) => Some(env),
            _ => None,
        }
    })
}

/// A pane's terminal read on a thread of its own (its output, then its
/// end, told to the loop) and written to on another: what is typed goes
/// to the returned sender.
fn start_io(tx: &Sender<Ev>, pane: u64, pty: &Local) -> Sender<Vec<u8>> {
    let tx2 = tx.clone();
    pty.read_on_thread(pane, move |out| {
        let _ = tx2.send(match out {
            crate::pty::Output::Bytes(id, b) => Ev::Out(id, b),
            _ => Ev::Ended(pane),
        });
    });
    let (keys, rx_keys) = channel::<Vec<u8>>();
    // Closed on exec: a server swapped for a newer one leaves none behind.
    let fd = unsafe { libc::fcntl(pty.fd, libc::F_DUPFD_CLOEXEC, 0) };
    std::thread::spawn(move || {
        while let Ok(b) = rx_keys.recv() {
            Local::write_fd(fd, &b);
        }
        unsafe { libc::close(fd) };
    });
    keys
}

impl Log {
    /// A log as `encode` wrote it: its entries kept as they were.
    fn decode(bytes: &[u8]) -> Log {
        let mut log = Log::default();
        let mut first = true;
        for e in super::log_entries(bytes) {
            match e {
                super::Entry::Size(c, r) if first => log.base = (c, r),
                super::Entry::Size(c, r) => log.entries.push_back(Kept::Size(c, r)),
                super::Entry::Time(ms) => log.entries.push_back(Kept::Time(ms)),
                super::Entry::Bytes(b) => {
                    log.bytes += b.len();
                    log.entries.push_back(Kept::Bytes(b.to_vec()));
                }
            }
            first = false;
        }
        log
    }
}

/// What a server hands the newer one it is swapped for (`jot restart`):
/// written to a file, read by the new one before it goes on.
const HANDOVER: &str = "jot-handover-1";

/// File descriptors a handover passes: kept open across exec.
fn keep_on_exec(fd: i32, keep: bool) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        libc::fcntl(fd, libc::F_SETFD, if keep { flags & !libc::FD_CLOEXEC } else { flags | libc::FD_CLOEXEC });
    }
}

impl Server {
    /// The server swapped for the jot at `exe`, in this process (exec):
    /// the shells stay its children, their terminals and the socket are
    /// passed open, and what it keeps of each pane is written for the new
    /// one. The windows are told first, and reach it again. Returns only
    /// if that failed, the server going on as it was (the windows reach
    /// it again all the same).
    fn handover(&mut self, exe: &str, listener: i32, path: &Path) -> io::Error {
        // The windows, told, are let go: their workspaces kept as those of
        // windows gone, for each to take back.
        for c in std::mem::take(&mut self.windows) {
            self.send(c, ToClient::Restarting);
            if let Some(l) = self.layouts.remove(&c) {
                self.left.push((self.names.get(&c).copied().unwrap_or(0), l));
            }
        }
        for (_, st) in self.conns.drain() {
            let _ = st.shutdown(std::net::Shutdown::Both);
        }
        self.names.clear();
        self.sharing.clear();
        self.numbering.clear();
        self.seen.clear();
        for p in self.panes.values_mut() {
            p.viewers.clear();
            p.sizes.clear();
        }
        let mut o = Out::default();
        o.str(HANDOVER).u64(self.next_pane).u32(listener as u32).u32(self.left.len() as u32);
        for (name, l) in &self.left {
            o.u64(*name).str(l);
        }
        o.u32(self.panes.len() as u32);
        for (id, p) in &self.panes {
            o.u64(*id).u32(p.pty.fd as u32).u32(p.pty.pid as u32).str(&p.cmd).bytes(&p.log.encode()).bytes(&p.tail).u8(p.at_prompt as u8).queue(&p.queue);
            match &p.tee {
                Some(t) => o.u8(1).tee(&t.state).u64(t.told),
                None => o.u8(0),
            };
            o.u32(p.windows.len() as u32);
            for w in &p.windows {
                o.u64(*w);
            }
            o.u16(p.size.0).u16(p.size.1).u32(p.size.2 .0).u32(p.size.2 .1);
        }
        // The shared workspaces, after what an older jot reads.
        o.u32(self.shared.len() as u32);
        for w in &self.shared {
            o.str(&toml::to_string(w).unwrap_or_default());
        }
        // The terminals' numbers, after those.
        o.u32(self.panes.len() as u32);
        for (id, p) in &self.panes {
            o.u64(*id).u32(p.number);
        }
        let file = super::dir().join(format!("handover-{}", std::process::id()));
        if let Err(e) = std::fs::write(&file, &o.0) {
            return e;
        }
        let fds: Vec<i32> = std::iter::once(listener).chain(self.panes.values().map(|p| p.pty.fd)).collect();
        for fd in &fds {
            keep_on_exec(*fd, true);
        }
        let c = |s: &str| std::ffi::CString::new(s).unwrap_or_default();
        let args = [c("jot"), c("serve"), c(&path.to_string_lossy()), c("--handover"), c(&file.to_string_lossy())];
        let mut argv: Vec<*const libc::c_char> = args.iter().map(|a| a.as_ptr()).collect();
        argv.push(std::ptr::null());
        let exe = c(exe);
        unsafe { libc::execv(exe.as_ptr(), argv.as_ptr()) };
        let e = io::Error::last_os_error();
        for fd in &fds {
            keep_on_exec(*fd, false);
        }
        let _ = std::fs::remove_file(&file);
        e
    }
}

/// A server handed over by the one before it: its panes running, its
/// socket listening, its windows' workspaces waiting for them.
fn resume(file: &Path, tx: &Sender<Ev>) -> io::Result<(Server, i32)> {
    let bytes = std::fs::read(file)?;
    let _ = std::fs::remove_file(file);
    let mut i = In(&bytes);
    if i.str()? != HANDOVER {
        return Err(io::Error::other("not a handover this jot reads"));
    }
    let next_pane = i.u64()?;
    let listener = i.u32()? as i32;
    let mut left = Vec::new();
    for _ in 0..i.u32()? {
        left.push((i.u64()?, i.str()?));
    }
    let mut panes = HashMap::new();
    for _ in 0..i.u32()? {
        let id = i.u64()?;
        let pty = Local { fd: i.u32()? as i32, pid: i.u32()? as i32 };
        keep_on_exec(pty.fd, false);
        let (cmd, log, tail, at_prompt, queue) = (i.str()?, Log::decode(&i.bytes()?), i.bytes()?, i.u8()? != 0, i.queue()?);
        let tee = if i.u8()? != 0 { Some((i.tee()?, i.u64()?)) } else { None };
        let windows = (0..i.u32()?).map(|_| i.u64()).collect::<io::Result<Vec<_>>>()?;
        let size = (i.u16()?, i.u16()?, (i.u32()?, i.u32()?));
        // Its /tee written on to the end of its file.
        let tee = tee.and_then(|(state, told)| std::fs::OpenOptions::new().create(true).append(true).open(&state.path).ok().map(|file| Tee { state, file, told }));
        let keys = start_io(tx, id, &pty);
        panes.insert(id, Pane { pty, keys, cmd, log, raw: None, tail, at_prompt, queue, tee, proc: None, viewers: Vec::new(), windows, sizes: HashMap::new(), size, number: 0 });
    }
    // The shared workspaces, from a jot that shares them.
    let shared = (0..i.u32().unwrap_or(0)).map_while(|_| i.str().ok()).filter_map(|t| toml::from_str::<crate::persist::WorkspaceSpec>(&t).ok()).collect();
    // The terminals' numbers, from a jot that numbers them; any without one
    // (from a jot before) numbered as a new one would be, in the order they
    // were made.
    for _ in 0..i.u32().unwrap_or(0) {
        let (Ok(id), Ok(n)) = (i.u64(), i.u32()) else { break };
        if let Some(p) = panes.get_mut(&id) {
            p.number = n;
        }
    }
    let mut ids: Vec<u64> = panes.keys().copied().collect();
    ids.sort();
    for id in ids {
        if panes[&id].number == 0 {
            let n = (1..).find(|n| !panes.values().any(|p| p.number == *n)).unwrap_or(1);
            if let Some(p) = panes.get_mut(&id) {
                p.number = n;
            }
        }
    }
    keep_on_exec(listener, false);
    Ok((Server { panes, conns: HashMap::new(), windows: Vec::new(), layouts: HashMap::new(), names: HashMap::new(), left, next_pane, desktop: None, abouts: HashMap::new(), shared, sharing: Vec::new(), seen: HashMap::new(), changed_by: HashMap::new(), numbering: Vec::new() }, listener))
}

/// The session's names of the panes a window's layout has (its workspaces'
/// and its popped-out ones).
fn layout_ids(text: &str) -> Vec<u64> {
    let Ok(s) = toml::from_str::<crate::persist::Session>(text) else { return Vec::new() };
    s.workspaces.iter().filter_map(|w| w.layout.as_ref()).chain(s.popouts.iter()).flat_map(crate::persist::ids_in_order).collect()
}

impl Server {
    /// The session's state in full (jot status -v): the workspaces it
    /// shares, each window (connection, name, what it is, whether it
    /// shares, the shared ones it has seen, the workspace it shows), each
    /// terminal (who shows it, whose it is, its size), and the windows gone
    /// whose workspaces wait.
    fn detail(&self) -> String {
        use crate::persist::{shape, Session};
        let mut out = String::from("-- shared workspaces\n");
        for w in &self.shared {
            let by = w.id.and_then(|id| self.changed_by.get(&id)).map_or(String::new(), |c| format!(", last by window {c}"));
            out += &format!("   {}  id {:x} rev {}{by}\n      {}\n", w.name, w.id.unwrap_or(0), w.rev.unwrap_or(0), w.layout.as_ref().map(shape).unwrap_or_default());
        }
        out += "-- windows (connections)\n";
        for c in &self.windows {
            let name = self.names.get(c).copied().unwrap_or(0);
            let sharing = if self.sharing.contains(c) { "shares" } else { "does not share" };
            let seen = self.seen.get(c).map(|s| s.iter().map(|id| format!("{id:x}")).collect::<Vec<_>>().join(" ")).unwrap_or_default();
            out += &format!("   window {c}  name {name:x}  {}  {sharing}  seen [{seen}]\n", self.abouts.get(c).map_or("", String::as_str));
            if let Some(s) = self.layouts.get(c).and_then(|l| toml::from_str::<Session>(l).ok()) {
                for (i, w) in s.workspaces.iter().enumerate() {
                    let here = if i == s.current { " (shown)" } else { "" };
                    out += &format!("      {}  id {:x} rev {}{here}  {}\n", w.name, w.id.unwrap_or(0), w.rev.unwrap_or(0), w.layout.as_ref().map(shape).unwrap_or_default());
                }
            }
        }
        out += "-- terminals\n";
        let mut ids: Vec<&u64> = self.panes.keys().collect();
        ids.sort();
        for id in ids {
            let p = &self.panes[id];
            let viewers = p.viewers.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(",");
            let windows = p.windows.iter().map(|w| format!("{w:x}")).collect::<Vec<_>>().join(",");
            let proc = p.pty.proc();
            out += &format!("   {id}  #{}  pid {}  {}x{}  shown by [{viewers}]  windows [{windows}]  {}\n", p.number, p.pty.pid, p.size.0, p.size.1, if proc.busy { proc.fg.clone() } else { crate::persist::shorten(Path::new(&proc.cwd)) });
        }
        if !self.left.is_empty() {
            out += "-- windows gone, their workspaces waiting\n";
            for (name, l) in &self.left {
                let s = toml::from_str::<Session>(l).ok();
                let names = s.map(|s| s.workspaces.iter().map(|w| w.name.clone()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
                out += &format!("   name {name:x}: {names}\n");
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    //! The session's part in sharing workspaces among windows: what it
    //! takes from a window's layout into the list it shares, and what it
    //! saves. Windows here are connection numbers with nothing on the other
    //! end: what the session would tell them goes nowhere.
    use super::*;
    use crate::persist::{Session, Spec, WorkspaceSpec};

    fn server() -> Server {
        Server { panes: HashMap::new(), conns: HashMap::new(), windows: Vec::new(), layouts: HashMap::new(), names: HashMap::new(), left: Vec::new(), next_pane: 1, desktop: None, abouts: HashMap::new(), shared: Vec::new(), sharing: Vec::new(), seen: HashMap::new(), changed_by: HashMap::new(), numbering: Vec::new() }
    }

    /// A window that shares workspaces, connected as `c`.
    fn window(s: &mut Server, c: u64) {
        s.windows.push(c);
        s.sharing.push(c);
        s.names.insert(c, 100 + c);
    }

    fn pane(id: u64) -> Spec {
        Spec::Pane { cmd: String::new(), cwd: None, pins: Vec::new(), empty: false, id: Some(id), host: None }
    }

    fn remote(host: &str, id: u64) -> Spec {
        Spec::Pane { cmd: String::new(), cwd: None, pins: Vec::new(), empty: false, id: Some(id), host: Some(host.into()) }
    }

    fn across(kids: Vec<Spec>, sizes: Vec<f32>) -> Spec {
        Spec::Across { v: kids, sizes }
    }

    fn ws(name: &str, id: u64, rev: u64, layout: Spec) -> WorkspaceSpec {
        WorkspaceSpec { name: name.into(), id: Some(id), rev: Some(rev), layout: Some(layout), ..Default::default() }
    }

    /// Window `c` says its workspaces (its layout), as it does after a
    /// change.
    fn says(s: &mut Server, c: u64, workspaces: Vec<WorkspaceSpec>) {
        let text = toml::to_string(&Session { workspaces, ..Default::default() }).unwrap();
        s.take_shared(c, &text);
        s.layouts.insert(c, text);
    }

    fn rev(s: &Server, id: u64) -> Option<u64> {
        s.shared.iter().find(|w| w.id == Some(id)).and_then(|w| w.rev)
    }

    fn shape(s: &Server, id: u64) -> String {
        s.shared.iter().find(|w| w.id == Some(id)).and_then(|w| w.layout.as_ref()).map(crate::persist::shape).unwrap_or_default()
    }

    #[test]
    fn a_window_s_new_workspace_is_shared() {
        let mut s = server();
        window(&mut s, 1);
        says(&mut s, 1, vec![ws("w1", 10, 0, pane(1))]);
        assert_eq!(s.shared.len(), 1);
        assert_eq!(rev(&s, 10), Some(1));
    }

    #[test]
    fn a_split_is_a_change() {
        let mut s = server();
        window(&mut s, 1);
        says(&mut s, 1, vec![ws("w1", 10, 0, pane(1))]);
        says(&mut s, 1, vec![ws("w1", 10, 1, across(vec![pane(1), pane(2)], vec![0.5, 0.5]))]);
        assert_eq!(rev(&s, 10), Some(2));
        assert_eq!(shape(&s, 10), "a(p1,p2)");
    }

    #[test]
    fn sizes_and_folders_are_each_window_s_own() {
        let mut s = server();
        window(&mut s, 1);
        says(&mut s, 1, vec![ws("w1", 10, 0, across(vec![pane(1), pane(2)], vec![0.5, 0.5]))]);
        let mut moved = ws("w1", 10, 1, across(vec![pane(1), pane(2)], vec![0.7, 0.3]));
        moved.cwd = Some("/tmp".into());
        says(&mut s, 1, vec![moved]);
        assert_eq!(rev(&s, 10), Some(1), "only sizes changed: no new rev");
    }

    #[test]
    fn a_rename_or_color_is_a_change() {
        let mut s = server();
        window(&mut s, 1);
        says(&mut s, 1, vec![ws("w1", 10, 0, pane(1))]);
        says(&mut s, 1, vec![ws("renamed", 10, 1, pane(1))]);
        assert_eq!(rev(&s, 10), Some(2));
        let mut colored = ws("renamed", 10, 2, pane(1));
        colored.accent = Some("teal".into());
        says(&mut s, 1, vec![colored]);
        assert_eq!(rev(&s, 10), Some(3));
    }

    #[test]
    fn another_window_s_stale_change_is_not_taken() {
        let mut s = server();
        window(&mut s, 1);
        window(&mut s, 2);
        says(&mut s, 1, vec![ws("w1", 10, 0, pane(1))]);
        says(&mut s, 2, vec![ws("w1", 10, 1, pane(1))]);
        says(&mut s, 1, vec![ws("w1", 10, 1, across(vec![pane(1), pane(2)], vec![]))]);
        // Window 2 had not seen the split yet: what it says is its echo.
        says(&mut s, 2, vec![ws("w1", 10, 1, pane(1))]);
        assert_eq!(shape(&s, 10), "a(p1,p2)");
        assert_eq!(rev(&s, 10), Some(2));
    }

    #[test]
    fn a_window_s_changes_one_after_another_are_all_taken() {
        let mut s = server();
        window(&mut s, 1);
        says(&mut s, 1, vec![ws("w1", 10, 0, pane(1))]);
        says(&mut s, 1, vec![ws("w1", 10, 1, across(vec![pane(1), pane(2)], vec![]))]);
        // Split again before it heard back about the first: still rev 1.
        says(&mut s, 1, vec![ws("w1", 10, 1, across(vec![pane(1), pane(2), pane(3)], vec![]))]);
        assert_eq!(shape(&s, 10), "a(p1,p2,p3)");
        assert_eq!(rev(&s, 10), Some(3));
    }

    #[test]
    fn closed_in_one_window_is_closed_for_all() {
        let mut s = server();
        window(&mut s, 1);
        says(&mut s, 1, vec![ws("w1", 10, 0, pane(1)), ws("w2", 20, 0, pane(2))]);
        says(&mut s, 1, vec![ws("w1", 10, 1, pane(1))]);
        assert!(rev(&s, 20).is_none());
        assert_eq!(s.shared.len(), 1);
    }

    #[test]
    fn a_closed_workspace_said_again_by_a_window_that_had_it_stays_closed() {
        let mut s = server();
        window(&mut s, 1);
        window(&mut s, 2);
        says(&mut s, 1, vec![ws("w1", 10, 0, pane(1)), ws("w2", 20, 0, pane(2))]);
        says(&mut s, 2, vec![ws("w1", 10, 1, pane(1)), ws("w2", 20, 1, pane(2))]);
        says(&mut s, 1, vec![ws("w1", 10, 1, pane(1))]);
        // Window 2 says it once more before it hears w2 was closed.
        says(&mut s, 2, vec![ws("w1", 10, 1, pane(1)), ws("w2", 20, 1, pane(2))]);
        assert!(rev(&s, 20).is_none(), "its echo, not a new one");
    }

    #[test]
    fn a_window_that_has_not_heard_of_a_new_workspace_does_not_close_it() {
        let mut s = server();
        window(&mut s, 1);
        window(&mut s, 2);
        says(&mut s, 2, vec![ws("w1", 10, 0, pane(1))]);
        says(&mut s, 1, vec![ws("w1", 10, 1, pane(1)), ws("w2", 20, 0, pane(2))]);
        // Window 2 says its workspaces before it hears of w2.
        says(&mut s, 2, vec![ws("w1", 10, 1, pane(1))]);
        assert_eq!(rev(&s, 20), Some(1));
    }

    #[test]
    fn a_new_workspace_goes_after_the_one_before_it() {
        let mut s = server();
        window(&mut s, 1);
        says(&mut s, 1, vec![ws("w1", 10, 0, pane(1)), ws("w3", 30, 0, pane(3))]);
        says(&mut s, 1, vec![ws("w1", 10, 1, pane(1)), ws("w2", 20, 0, pane(2)), ws("w3", 30, 1, pane(3))]);
        let order: Vec<u64> = s.shared.iter().filter_map(|w| w.id).collect();
        assert_eq!(order, vec![10, 20, 30]);
    }

    /// The bug a remote terminal clicked into a local split made: the
    /// workspace, mixing machines, was taken for closed.
    #[test]
    fn a_workspace_mixing_machines_is_shared_not_closed() {
        let mut s = server();
        window(&mut s, 1);
        says(&mut s, 1, vec![ws("w1", 10, 0, across(vec![pane(1), pane(2)], vec![]))]);
        says(&mut s, 1, vec![ws("w1", 10, 1, across(vec![pane(1), remote("skyfall", 2)], vec![]))]);
        assert_eq!(rev(&s, 10), Some(2), "kept, and changed");
        assert_eq!(shape(&s, 10), "a(p1,pskyfall:2)");
    }

    #[test]
    fn a_workspace_only_of_another_machine_s_terminals_is_still_this_one_s() {
        let mut s = server();
        window(&mut s, 1);
        says(&mut s, 1, vec![ws("w1", 10, 0, remote("skyfall", 7))]);
        assert_eq!(rev(&s, 10), Some(1));
    }

    #[test]
    fn a_workspace_another_machine_is_home_to_is_not_shared_here() {
        let mut s = server();
        window(&mut s, 1);
        let mut theirs = ws("w1", 10, 0, remote("skyfall", 7));
        theirs.home = Some("skyfall".into());
        says(&mut s, 1, vec![theirs]);
        assert!(s.shared.is_empty());
    }

    #[test]
    fn a_window_that_does_not_share_changes_nothing() {
        let mut s = server();
        window(&mut s, 1);
        s.windows.push(2);
        says(&mut s, 1, vec![ws("w1", 10, 0, pane(1))]);
        // What the handler does with a layout from a window that does not
        // share: kept as its own, not taken in.
        let text = toml::to_string(&Session { workspaces: vec![ws("old", 30, 0, pane(3))], ..Default::default() }).unwrap();
        if s.sharing.contains(&2) {
            s.take_shared(2, &text);
        }
        s.layouts.insert(2, text);
        assert_eq!(s.shared.len(), 1);
    }

    #[test]
    fn saved_once_with_each_window_showing_its_own() {
        let mut s = server();
        window(&mut s, 1);
        window(&mut s, 2);
        let both = |current: usize| {
            let text = toml::to_string(&Session { current, workspaces: vec![ws("w1", 10, 1, pane(1)), ws("w2", 20, 1, pane(2))], window: Some(crate::persist::WindowState { x: 0, y: 0, w: 800, h: 600, maximized: false, font_size: 14.0 }), ..Default::default() }).unwrap();
            text
        };
        says(&mut s, 1, vec![ws("w1", 10, 0, pane(1)), ws("w2", 20, 0, pane(2))]);
        s.layouts.insert(1, both(0));
        s.layouts.insert(2, both(1));
        s.seen.insert(2, vec![10, 20]);
        let saved = s.saved().expect("something to save");
        let ids: Vec<u64> = saved.workspaces.iter().filter_map(|w| w.id).collect();
        assert_eq!(ids, vec![10, 20], "each shared workspace once");
        assert_eq!(saved.windows.len(), 2);
        assert_eq!(saved.windows[0].workspaces.len(), 2, "the first window brings them back");
        assert!(saved.windows[1].workspaces.is_empty(), "the others get them from the session");
        let shows: Vec<Option<u64>> = saved.windows.iter().map(|w| w.current_id).collect();
        assert!(shows.contains(&Some(10)) && shows.contains(&Some(20)), "each the one it showed: {shows:?}");
    }
}

impl Server {
    /// The lowest number no terminal has, for a new one.
    fn free_number(&self) -> u32 {
        (1..).find(|n| !self.panes.values().any(|p| p.number == *n)).unwrap_or(1)
    }

    /// Terminals' numbers as a window says them (each pane and its number):
    /// each taken, one another terminal had swapping with it, so each
    /// number is one terminal's.
    fn renumber(&mut self, numbers: &[(u64, u32)]) {
        for &(pane, n) in numbers {
            if n == 0 {
                continue;
            }
            let Some(was) = self.panes.get(&pane).map(|p| p.number) else { continue };
            if let Some(other) = self.panes.values_mut().find(|p| p.number == n) {
                other.number = was;
            }
            if let Some(p) = self.panes.get_mut(&pane) {
                p.number = n;
            }
        }
    }

    /// Every terminal's number told to window `to`, or to every window
    /// that takes them (None).
    fn tell_numbers(&mut self, to: Option<u64>) {
        let mut numbers: Vec<(u64, u32)> = self.panes.iter().map(|(id, p)| (*id, p.number)).collect();
        numbers.sort();
        let windows: Vec<u64> = match to {
            Some(c) => self.numbering.iter().copied().filter(|w| *w == c).collect(),
            None => self.numbering.clone(),
        };
        for w in windows {
            self.send(w, ToClient::Numbers { numbers: numbers.clone() });
        }
    }
}
