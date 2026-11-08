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
        let waiting: Vec<u64> = left.iter().flat_map(|(_, l)| layout_ids(l)).collect();
        self.left = quake;
        self.left.extend(left);
        let more = self.left.iter().filter(|(w, _)| *w != super::QUAKE_WINDOW).count() as u32;
        // The quake window's panes and workspaces wait for the next one, and
        // the other windows' for theirs.
        let free = |id: &u64, p: &Pane| p.viewers.is_empty() && !p.windows.contains(&super::QUAKE_WINDOW) && !waiting.contains(id);
        let mut held: Vec<Held> = self.panes.iter().filter(|(id, p)| free(id, p)).map(|(id, p)| p.held(*id)).collect();
        held.sort_by_key(|h| h.pane);
        let name = self.names.get(&c).copied().unwrap_or(0);
        for (_, p) in self.panes.iter_mut().filter(|(id, p)| free(id, p)) {
            p.add_viewer(c, name);
        }
        let layout = mine
            .and_then(|(_, l)| toml::from_str::<crate::persist::Session>(&l).ok())
            .map(|mut s| {
                s.more = more;
                s
            })
            .and_then(|s| toml::to_string(&s).ok())
            .unwrap_or_default();
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
                if ids.is_empty() || ids.iter().all(|id| self.panes.get(id).is_some_and(|p| p.viewers.contains(&c))) {
                    named.extend(&ids);
                    continue;
                }
                named.extend(&ids);
                // Shown elsewhere only if its panes are a live window's.
                let shown = elsewhere && ids.iter().any(|id| self.panes.get(id).is_some_and(|p| p.viewers.iter().any(|v| *v != c)));
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
        ToClient::Listing { workspaces }
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
        let taken: Vec<u64> = held.iter().map(|h| h.pane).collect();
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
        let open = self.layouts.iter().map(|(c, l)| (self.names.get(c).copied().unwrap_or(0), l));
        let all: Vec<(u64, &String)> = open.chain(self.left.iter().map(|(w, l)| (*w, l))).collect();
        let Some(mut s) = merge(all.iter().map(|(_, l)| l.as_str())) else { return };
        let mut groups: Vec<(u64, crate::persist::Session)> = all
            .iter()
            .filter(|(w, _)| *w != super::QUAKE_WINDOW)
            .filter_map(|(w, l)| toml::from_str::<crate::persist::Session>(l).ok().map(|s| (*w, s)))
            .filter(|(_, s)| !s.workspaces.is_empty() || !s.popouts.is_empty())
            .collect();
        groups.sort_by_key(|(w, _)| *w);
        if groups.len() > 1 {
            let quake = merge(all.iter().filter(|(w, _)| *w == super::QUAKE_WINDOW).map(|(_, l)| l.as_str()));
            if let Some(q) = quake {
                groups[0].1.workspaces.extend(q.workspaces);
            }
            s.windows = groups.into_iter().map(|(_, s)| s).collect();
        }
        crate::persist::save_session(&s);
    }

    fn info(&self) -> ToClient {
        let names = merge(self.layouts.values().map(String::as_str).chain(self.left.iter().map(|(_, l)| l.as_str()))).map(|s| s.workspaces.iter().map(|w| w.name.clone()).filter(|n| !n.is_empty()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        ToClient::Info { attached: !self.windows.is_empty(), panes: self.panes.len() as u32, names, version: crate::version(), windows: self.windows.len() as u32 }
    }

    /// A new pane for window `c`: its name and its shell's process id (-1 if
    /// it could not start).
    fn spawn(&mut self, tx: &Sender<Ev>, c: u64, cmd: String, cwd: String, cols: u16, rows: u16, px: (u32, u32)) -> (u64, i32) {
        let pane = self.next_pane;
        self.next_pane += 1;
        let desktop = self.conns.get(&c).map(desktop_of).unwrap_or_default();
        let pid = match Local::spawn(&cmd, Path::new(&cwd), cols, rows, px, &desktop) {
            Ok(pty) => {
                let pid = pty.pid;
                let keys = start_io(tx, pane, &pty);
                let mut log = Log::default();
                log.size(cols, rows);
                self.panes.insert(pane, Pane { pty, keys, cmd, log, raw: None, tail: Vec::new(), at_prompt: false, queue: Queue::default(), tee: None, proc: None, viewers: vec![c], windows: vec![self.names.get(&c).copied().unwrap_or(0)], sizes: HashMap::from([(c, (cols, rows, px))]), size: (cols, rows, px) });
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
            Ok(l) => (Server { panes: HashMap::new(), conns: HashMap::new(), windows: Vec::new(), layouts: HashMap::new(), names: HashMap::new(), left: Vec::new(), next_pane: 1 }, l),
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
                    s.tell_sizes(c, &panes);
                }
                ToServer::Release { pane } => {
                    if let Some(p) = s.panes.get_mut(&pane) {
                        if p.drop_viewer(c) {
                            let m = p.size_msg(pane);
                            s.tell(pane, m);
                        }
                    }
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

/// A pane's terminal read on a thread of its own (its output, then its
/// end, told to the loop) and written to on another: what is typed goes
/// to the returned sender.

/// What a pane's programs need of the desktop: the display, the session's
/// bus and what kind it is. The session has them as whatever started it
/// had them, and that may have been an SSH login (`jot bridge`, a
/// window's `jot restart` over SSH), with none: a window that asks for a
/// pane gives it its own.
const DESKTOP: [&str; 8] = ["DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY", "XDG_SESSION_TYPE", "XDG_CURRENT_DESKTOP", "XDG_SESSION_DESKTOP", "DESKTOP_SESSION", "DBUS_SESSION_BUS_ADDRESS"];

/// The desktop of the process at the other end of `s`, if it is on one
/// (a window here; not `jot bridge`, for a window on another machine,
/// whose panes keep the session's).
fn desktop_of(s: &UnixStream) -> Vec<(std::ffi::CString, Option<std::ffi::CString>)> {
    use std::os::fd::AsRawFd;
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let got = unsafe { libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut libc::c_void, &mut len) };
    if got != 0 || cred.pid <= 0 {
        return Vec::new();
    }
    let Ok(raw) = std::fs::read(format!("/proc/{}/environ", cred.pid)) else { return Vec::new() };
    let vars: HashMap<&[u8], &[u8]> = raw.split(|b| *b == 0).filter_map(|kv| kv.iter().position(|b| *b == b'=').map(|i| (&kv[..i], &kv[i + 1..]))).collect();
    let on = |k: &str| vars.get(k.as_bytes()).is_some_and(|v| !v.is_empty());
    if !on("DISPLAY") && !on("WAYLAND_DISPLAY") {
        return Vec::new();
    }
    DESKTOP.iter().filter_map(|k| Some((std::ffi::CString::new(*k).ok()?, vars.get(k.as_bytes()).and_then(|v| std::ffi::CString::new(*v).ok())))).collect()
}
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
        panes.insert(id, Pane { pty, keys, cmd, log, raw: None, tail, at_prompt, queue, tee, proc: None, viewers: Vec::new(), windows, sizes: HashMap::new(), size });
    }
    keep_on_exec(listener, false);
    Ok((Server { panes, conns: HashMap::new(), windows: Vec::new(), layouts: HashMap::new(), names: HashMap::new(), left, next_pane }, listener))
}

/// The session's names of the panes a window's layout has (its workspaces'
/// and its popped-out ones).
fn layout_ids(text: &str) -> Vec<u64> {
    let Ok(s) = toml::from_str::<crate::persist::Session>(text) else { return Vec::new() };
    s.workspaces.iter().filter_map(|w| w.layout.as_ref()).chain(s.popouts.iter()).flat_map(crate::persist::ids_in_order).collect()
}
