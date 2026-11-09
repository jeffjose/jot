//! A session (`jot serve`) and windows sharing its workspaces, the windows
//! played here: each a connection speaking the session's protocol, as a
//! window does, saying its workspaces and hearing what the session says
//! back. Each test has a session of its own, in a folder of its own.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const PROTO: u32 = 7;
const CAN_SHARE: u32 = 1;

// What a window says (ToServer) and hears (ToClient), by their tags.
const HELLO: u8 = 0;
const SPAWN: u8 = 1;
const LAYOUT: u8 = 5;
const WELCOME: u8 = 0;
const SPAWNED: u8 = 2;
const WORKSPACES: u8 = 15;

/// A session of its own: `jot serve`, its socket and state in a folder
/// that goes with it.
struct Session {
    dir: PathBuf,
    serve: Child,
}

impl Session {
    fn start() -> Session {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // Short: a socket's path has room for little more than 100 bytes.
        let dir = std::env::temp_dir().join(format!("jot-t{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("state")).unwrap();
        let serve = Command::new(env!("CARGO_BIN_EXE_jot")).arg("serve").arg(dir.join("jot-sock")).envs(env(&dir)).spawn().expect("jot serve");
        let s = Session { dir, serve };
        let deadline = Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(s.socket()).is_err() {
            assert!(Instant::now() < deadline, "the session did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        s
    }

    fn socket(&self) -> PathBuf {
        self.dir.join("jot-sock")
    }

    /// A window, `name` for as long as it is open, sharing workspaces or
    /// not (an older jot's).
    fn window(&self, name: u64, sharing: bool) -> Window {
        Window::hello(&self.socket(), name, false, if sharing { CAN_SHARE } else { 0 })
    }

    /// The `jot` command (status, restart) run at this session.
    fn jot(&self, args: &[&str]) -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_jot")).args(args).envs(env(&self.dir)).output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.serve.kill();
        let _ = self.serve.wait();
        // `jot restart` swaps the server in the same process; one that ended
        // up elsewhere ends with its last pane, which ends with its shell.
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// What a session and the jot commands run at it are given: the session's
/// socket in `dir` (XDG_RUNTIME_DIR/jot-UID/session.sock, for the commands),
/// its state in `dir`, a plain shell for its panes.
fn env(dir: &Path) -> Vec<(&'static str, PathBuf)> {
    let uid = unsafe { libc_getuid() };
    let run = dir.join("run");
    let _ = std::fs::create_dir_all(run.join(format!("jot-{uid}")));
    // The commands find the session by XDG_RUNTIME_DIR: the socket put
    // where they look.
    let link = run.join(format!("jot-{uid}")).join("session.sock");
    if !link.exists() {
        let _ = std::os::unix::fs::symlink(dir.join("jot-sock"), &link);
    }
    vec![("XDG_RUNTIME_DIR", run), ("XDG_STATE_HOME", dir.join("state")), ("SHELL", PathBuf::from("/bin/sh"))]
}

extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
}

/// A window, played: a connection that has said hello.
struct Window {
    s: UnixStream,
    /// What it said hello with, to say it again on reaching the session again.
    name: u64,
    /// What the session said when it came: the layout it gave, and the
    /// terminals it gave with it.
    layout: String,
    given: Vec<u64>,
}

impl Window {
    fn hello(socket: &Path, name: u64, again: bool, caps: u32) -> Window {
        let mut s = UnixStream::connect(socket).unwrap();
        let mut o = vec![HELLO];
        o.extend(PROTO.to_le_bytes());
        o.extend(name.to_le_bytes());
        o.push(again as u8);
        o.extend(0u32.to_le_bytes());
        put_str(&mut o, "jot test · test");
        o.extend(caps.to_le_bytes());
        send(&mut s, &o);
        let mut w = Window { s, name, layout: String::new(), given: Vec::new() };
        let f = w.expect(WELCOME, Duration::from_secs(5)).expect("welcome");
        let mut i = In(&f[1..]);
        w.layout = i.str();
        w.given = held(&mut i);
        w
    }

    /// A terminal started in the session: its name there.
    fn spawn(&mut self) -> u64 {
        let mut o = vec![SPAWN];
        put_str(&mut o, "");
        put_str(&mut o, "/tmp");
        o.extend(80u16.to_le_bytes());
        o.extend(24u16.to_le_bytes());
        o.extend(0u32.to_le_bytes());
        o.extend(0u32.to_le_bytes());
        send(&mut self.s, &o);
        let f = self.expect(SPAWNED, Duration::from_secs(5)).expect("spawned");
        In(&f[1..]).u64()
    }

    /// Its workspaces, said: `workspaces` as a session file has them.
    fn says(&mut self, workspaces: &str) {
        let mut o = vec![LAYOUT];
        put_str(&mut o, &format!("current = 0\n{workspaces}"));
        send(&mut self.s, &o);
    }

    /// The shared workspaces the session tells it next (each as written),
    /// and the terminals given with them; None if it says none in `wait`.
    fn hears_workspaces(&mut self, wait: Duration) -> Option<(Vec<String>, Vec<u64>)> {
        let f = self.expect(WORKSPACES, wait)?;
        let mut i = In(&f[1..]);
        let n = i.u32();
        let specs = (0..n).map(|_| i.str()).collect();
        Some((specs, held(&mut i)))
    }

    /// The last list it is told in `wait`: what it shows once the session
    /// is done telling.
    fn settles(&mut self, wait: Duration) -> Option<Vec<String>> {
        let mut last = None;
        while let Some((specs, _)) = self.hears_workspaces(wait) {
            last = Some(specs);
        }
        last
    }

    /// Frames read until one with `tag`, others passed by; None in `wait`.
    fn expect(&mut self, tag: u8, wait: Duration) -> Option<Vec<u8>> {
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.checked_duration_since(Instant::now())?;
            self.s.set_read_timeout(Some(left.max(Duration::from_millis(1)))).unwrap();
            let mut len = [0u8; 4];
            if self.s.read_exact(&mut len).is_err() {
                return None;
            }
            let mut f = vec![0u8; u32::from_le_bytes(len) as usize];
            self.s.set_read_timeout(None).unwrap();
            self.s.read_exact(&mut f).unwrap();
            if f.first() == Some(&tag) {
                return Some(f);
            }
        }
    }
}

fn send(s: &mut UnixStream, body: &[u8]) {
    let mut f = (body.len() as u32).to_le_bytes().to_vec();
    f.extend_from_slice(body);
    s.write_all(&f).unwrap();
}

fn put_str(o: &mut Vec<u8>, v: &str) {
    o.extend((v.len() as u32).to_le_bytes());
    o.extend(v.as_bytes());
}

struct In<'a>(&'a [u8]);

impl In<'_> {
    fn take(&mut self, n: usize) -> &[u8] {
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        a
    }
    fn u8(&mut self) -> u8 {
        self.take(1)[0]
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take(4).try_into().unwrap())
    }
    fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.take(8).try_into().unwrap())
    }
    fn bytes(&mut self) -> Vec<u8> {
        let n = self.u32() as usize;
        self.take(n).to_vec()
    }
    fn str(&mut self) -> String {
        String::from_utf8_lossy(&self.bytes()).into_owned()
    }
}

/// Terminals given (Held), by their names; what else comes with each, read
/// past.
fn held(i: &mut In) -> Vec<u64> {
    let n = i.u32();
    (0..n)
        .map(|_| {
            let pane = i.u64();
            i.u32();
            i.str();
            i.bytes();
            if i.u8() != 0 {
                i.str();
                i.u64();
                i.u8();
                i.u8();
            }
            let items = i.u32();
            for _ in 0..items {
                i.str();
            }
            i.u8();
            let after = i.u32();
            for _ in 0..after {
                i.u64();
                i.str();
            }
            i.u8();
            pane
        })
        .collect()
}

/// A workspace as a session file has it, its layout given as TOML.
fn ws(name: &str, id: u64, rev: u64, layout: &str) -> String {
    format!("[[workspace]]\nname = \"{name}\"\nid = {id}\nrev = {rev}\n[workspace.layout]\n{layout}\n")
}

fn one(pane: u64) -> String {
    format!("cmd = \"\"\nid = {pane}")
}

fn side_by_side(panes: &[(Option<&str>, u64)]) -> String {
    let each: Vec<String> = panes.iter().map(|(host, id)| match host {
        Some(h) => format!("{{ cmd = \"\", id = {id}, host = \"{h}\" }}"),
        None => format!("{{ cmd = \"\", id = {id} }}"),
    }).collect();
    format!("v = [{}]", each.join(", "))
}

fn named(specs: &[String], name: &str) -> Option<String> {
    specs.iter().find(|s| s.contains(&format!("name = \"{name}\""))).cloned()
}

const A_WHILE: Duration = Duration::from_millis(600);

#[test]
fn a_new_window_has_every_shared_workspace_and_its_terminals() {
    let s = Session::start();
    let mut a = s.window(1, true);
    let t = a.spawn();
    a.says(&ws("w1", 10, 0, &one(t)));
    assert!(a.settles(A_WHILE).is_some_and(|l| named(&l, "w1").is_some()));
    let b = s.window(2, true);
    assert!(b.layout.contains("name = \"w1\""), "its layout: {}", b.layout);
    assert!(b.given.contains(&t), "the terminal in it, given: {:?}", b.given);
}

#[test]
fn a_workspace_made_in_one_window_comes_to_the_other() {
    let s = Session::start();
    let mut a = s.window(1, true);
    let t1 = a.spawn();
    a.says(&ws("w1", 10, 0, &one(t1)));
    let mut b = s.window(2, true);
    b.settles(A_WHILE);
    let t2 = a.spawn();
    a.says(&(ws("w1", 10, 1, &one(t1)) + &ws("w2", 20, 0, &one(t2))));
    let (specs, given) = b.hears_workspaces(Duration::from_secs(2)).expect("told");
    assert!(named(&specs, "w2").is_some(), "{specs:?}");
    assert!(given.contains(&t2), "with its terminal: {given:?}");
}

#[test]
fn a_split_in_one_window_comes_to_the_other() {
    let s = Session::start();
    let mut a = s.window(1, true);
    let t1 = a.spawn();
    a.says(&ws("w1", 10, 0, &one(t1)));
    let mut b = s.window(2, true);
    b.settles(A_WHILE);
    let t2 = a.spawn();
    a.says(&ws("w1", 10, 1, &side_by_side(&[(None, t1), (None, t2)])));
    let specs = b.settles(A_WHILE).expect("told");
    let w1 = named(&specs, "w1").unwrap();
    assert!(w1.contains(&format!("id = {t2}")), "{w1}");
}

/// The bug: a remote terminal brought into a split made the workspace mix
/// machines, and the session took it for closed: every window lost it.
#[test]
fn a_workspace_mixing_machines_stays_and_comes_to_the_other_window() {
    let s = Session::start();
    let mut a = s.window(1, true);
    let t1 = a.spawn();
    let t2 = a.spawn();
    a.says(&ws("w1", 10, 0, &side_by_side(&[(None, t1), (None, t2)])));
    let mut b = s.window(2, true);
    b.settles(A_WHILE);
    a.says(&ws("w1", 10, 1, &side_by_side(&[(None, t1), (Some("skyfall"), 7)])));
    let specs = b.settles(A_WHILE).expect("told");
    let w1 = named(&specs, "w1").expect("not closed");
    assert!(w1.contains("host = \"skyfall\""), "{w1}");
    assert!(a.settles(A_WHILE).is_some_and(|l| named(&l, "w1").is_some()), "nor for the window that made it");
}

#[test]
fn closed_in_one_window_is_gone_from_the_other() {
    let s = Session::start();
    let mut a = s.window(1, true);
    let t1 = a.spawn();
    let t2 = a.spawn();
    a.says(&(ws("w1", 10, 0, &one(t1)) + &ws("w2", 20, 0, &one(t2))));
    let mut b = s.window(2, true);
    b.settles(A_WHILE);
    a.says(&ws("w1", 10, 1, &one(t1)));
    let specs = b.settles(A_WHILE).expect("told");
    assert!(named(&specs, "w2").is_none(), "{specs:?}");
    assert!(named(&specs, "w1").is_some());
}

#[test]
fn a_window_from_before_sharing_is_not_told() {
    let s = Session::start();
    let mut old = s.window(1, false);
    let mut a = s.window(2, true);
    let t = a.spawn();
    a.says(&ws("w1", 10, 0, &one(t)));
    assert!(a.settles(A_WHILE).is_some());
    assert!(old.hears_workspaces(A_WHILE).is_none(), "it would not know the message");
}

#[test]
fn shared_workspaces_go_over_jot_restart() {
    let s = Session::start();
    let mut a = s.window(1, true);
    let t = a.spawn();
    a.says(&ws("w1", 10, 0, &one(t)));
    a.settles(A_WHILE);
    let out = s.jot(&["restart"]);
    assert!(out.contains("now"), "{out}");
    // Reached again, as a window does after a restart.
    let mut again = Window::hello(&s.socket(), a.name, true, CAN_SHARE);
    let specs = again.settles(A_WHILE).expect("told again");
    assert!(named(&specs, "w1").is_some(), "{specs:?}");
}

#[test]
fn status_shows_the_session_s_shared_workspaces() {
    let s = Session::start();
    let mut a = s.window(1, true);
    let t = a.spawn();
    a.says(&ws("w1", 10, 0, &side_by_side(&[(None, t), (Some("skyfall"), 7)])));
    a.settles(A_WHILE);
    let out = s.jot(&["status", "-v"]);
    assert!(out.contains("-- shared workspaces"), "{out}");
    assert!(out.contains("w1"), "{out}");
    assert!(out.contains("pskyfall:7"), "its layout, the remote terminal in it: {out}");
}

// Terminals' numbers, the same in every window: the session numbers its
// terminals and tells every window that takes them (CAN_NUMBER).

const CAN_NUMBER: u32 = 2;
const CLOSE: u8 = 4;
const RENUMBER: u8 = 15;
const NUMBERS: u8 = 16;

impl Window {
    /// The numbers the session tells it next, each terminal's; the last it
    /// tells in `wait`.
    fn numbers(&mut self, wait: Duration) -> Option<Vec<(u64, u32)>> {
        let mut last = None;
        while let Some(f) = self.expect(NUMBERS, wait) {
            let mut i = In(&f[1..]);
            let n = i.u32();
            last = Some((0..n).map(|_| (i.u64(), i.u32())).collect());
        }
        last
    }

    fn close(&mut self, pane: u64) {
        let mut o = vec![CLOSE];
        o.extend(pane.to_le_bytes());
        send(&mut self.s, &o);
    }

    fn renumber(&mut self, numbers: &[(u64, u32)]) {
        let mut o = vec![RENUMBER];
        o.extend((numbers.len() as u32).to_le_bytes());
        for (p, n) in numbers {
            o.extend(p.to_le_bytes());
            o.extend(n.to_le_bytes());
        }
        send(&mut self.s, &o);
    }
}

fn number_of(numbers: &[(u64, u32)], pane: u64) -> Option<u32> {
    numbers.iter().find(|(p, _)| *p == pane).map(|(_, n)| *n)
}

#[test]
fn terminals_are_numbered_one_two_three_and_every_window_hears_the_same() {
    let s = Session::start();
    let mut a = Window::hello(&s.socket(), 1, false, CAN_SHARE | CAN_NUMBER);
    let t1 = a.spawn();
    let t2 = a.spawn();
    let t3 = a.spawn();
    let mine = a.numbers(A_WHILE).expect("told");
    assert_eq!([t1, t2, t3].map(|t| number_of(&mine, t)), [Some(1), Some(2), Some(3)]);
    // Another window, and the quake window (which does not share
    // workspaces, but takes numbers): the same.
    let mut b = Window::hello(&s.socket(), 2, false, CAN_SHARE | CAN_NUMBER);
    let mut quake = Window::hello(&s.socket(), 3, false, CAN_NUMBER);
    assert_eq!(b.numbers(A_WHILE).as_ref(), Some(&mine));
    assert_eq!(quake.numbers(A_WHILE).as_ref(), Some(&mine));
}

#[test]
fn a_terminal_gone_frees_its_number_for_the_next() {
    let s = Session::start();
    let mut a = Window::hello(&s.socket(), 1, false, CAN_SHARE | CAN_NUMBER);
    let _t1 = a.spawn();
    let t2 = a.spawn();
    let _t3 = a.spawn();
    a.close(t2);
    let left = a.numbers(A_WHILE).expect("told");
    assert_eq!(number_of(&left, t2), None);
    let t4 = a.spawn();
    let now = a.numbers(A_WHILE).expect("told");
    assert_eq!(number_of(&now, t4), Some(2), "the lowest free: {now:?}");
}

#[test]
fn a_window_s_renumbering_reaches_every_window() {
    let s = Session::start();
    let mut a = Window::hello(&s.socket(), 1, false, CAN_SHARE | CAN_NUMBER);
    let t1 = a.spawn();
    let t2 = a.spawn();
    let mut b = Window::hello(&s.socket(), 2, false, CAN_SHARE | CAN_NUMBER);
    b.numbers(A_WHILE);
    // A row moved in A's sidebar: t2 first.
    a.renumber(&[(t2, 1), (t1, 2)]);
    let told = b.numbers(A_WHILE).expect("told");
    assert_eq!((number_of(&told, t2), number_of(&told, t1)), (Some(1), Some(2)));
    // Taking a number another has swaps them: never two of one.
    a.renumber(&[(t1, 1)]);
    let told = b.numbers(A_WHILE).expect("told");
    assert_eq!((number_of(&told, t1), number_of(&told, t2)), (Some(1), Some(2)));
}

#[test]
fn numbers_go_over_jot_restart() {
    let s = Session::start();
    let mut a = Window::hello(&s.socket(), 1, false, CAN_SHARE | CAN_NUMBER);
    let t1 = a.spawn();
    let t2 = a.spawn();
    a.renumber(&[(t2, 1), (t1, 2)]);
    a.numbers(A_WHILE);
    s.jot(&["restart"]);
    let mut again = Window::hello(&s.socket(), 1, true, CAN_SHARE | CAN_NUMBER);
    let told = again.numbers(A_WHILE).expect("told again");
    assert_eq!((number_of(&told, t2), number_of(&told, t1)), (Some(1), Some(2)));
}

#[test]
fn a_window_from_before_numbering_is_not_told() {
    let s = Session::start();
    let mut old = Window::hello(&s.socket(), 1, false, CAN_SHARE);
    old.spawn();
    assert!(old.numbers(A_WHILE).is_none(), "it would not know the message");
}

const CAN_REOPEN: u32 = 4;
const REOPEN_ASK: u8 = 16;
const REOPEN: u8 = 17;

/// `jot restart --all`'s word to the session, said apart from any window.
fn ask_reopen(s: &Session, exe: &str) {
    let mut c = UnixStream::connect(s.socket()).unwrap();
    let mut o = vec![REOPEN_ASK];
    put_str(&mut o, exe);
    send(&mut c, &o);
}

#[test]
fn reopen_is_told_to_the_windows_that_can_and_not_to_older_ones() {
    let s = Session::start();
    let mut a = Window::hello(&s.socket(), 1, false, CAN_SHARE | CAN_REOPEN);
    let mut old = s.window(2, true);
    ask_reopen(&s, "/some/jot");
    let f = a.expect(REOPEN, A_WHILE).expect("told to reopen");
    assert_eq!(In(&f[1..]).str(), "/some/jot");
    assert!(old.expect(REOPEN, A_WHILE).is_none(), "it would not know the message");
}

#[test]
fn a_window_reopened_takes_back_its_own_workspaces_and_terminals() {
    let s = Session::start();
    let mut a = Window::hello(&s.socket(), 1, false, CAN_SHARE | CAN_REOPEN);
    let mut b = Window::hello(&s.socket(), 3, false, CAN_SHARE | CAN_REOPEN);
    let (t1, t2) = (a.spawn(), a.spawn());
    a.says(&ws("w1", 10, 0, &side_by_side(&[(None, t1), (None, t2)])));
    a.settles(A_WHILE);
    b.settles(A_WHILE);
    let tb = b.spawn();
    b.says(&format!("{}{}", ws("w1", 10, 1, &side_by_side(&[(None, t1), (None, t2)])), ws("w2", 11, 0, &one(tb))));
    b.settles(A_WHILE);
    ask_reopen(&s, "/some/jot");
    a.expect(REOPEN, A_WHILE).expect("a told");
    b.expect(REOPEN, A_WHILE).expect("b told");
    // Each closes (exec) and opens again as the same window, in either order.
    let (an, bn) = (a.name, b.name);
    drop(b);
    drop(a);
    std::thread::sleep(Duration::from_millis(200));
    let b = Window::hello(&s.socket(), bn, true, CAN_SHARE | CAN_REOPEN);
    let a = Window::hello(&s.socket(), an, true, CAN_SHARE | CAN_REOPEN);
    // Every terminal of the shared workspaces is each one's.
    for t in [t1, t2, tb] {
        assert!(a.given.contains(&t) && b.given.contains(&t), "{t}: a {:?}, b {:?}", a.given, b.given);
    }
    assert!(b.layout.contains("w2"), "b's own workspaces: {}", b.layout);
}

const CAN_PROJECT: u32 = 8;
const NAME: u8 = 17;
const PROJECTS: u8 = 18;

impl Window {
    /// A terminal started in `cwd`: its name there.
    fn spawn_in(&mut self, cwd: &Path) -> u64 {
        let mut o = vec![SPAWN];
        put_str(&mut o, "");
        put_str(&mut o, &cwd.display().to_string());
        o.extend(80u16.to_le_bytes());
        o.extend(24u16.to_le_bytes());
        o.extend(0u32.to_le_bytes());
        o.extend(0u32.to_le_bytes());
        send(&mut self.s, &o);
        let f = self.expect(SPAWNED, Duration::from_secs(5)).expect("spawned");
        In(&f[1..]).u64()
    }

    /// What the session tells of `pane` (its project and name, as Told
    /// encodes them), the first time it does in `wait`; None if it does not.
    fn told_of(&mut self, pane: u64, wait: Duration) -> Option<String> {
        let deadline = Instant::now() + wait;
        loop {
            let f = self.expect(PROJECTS, deadline.checked_duration_since(Instant::now())?)?;
            let mut i = In(&f[1..]);
            let n = i.u32();
            for _ in 0..n {
                let (p, t) = (i.u64(), i.str());
                if p == pane {
                    return Some(t);
                }
            }
        }
    }

    fn names(&mut self, pane: u64, name: &str) {
        let mut o = vec![NAME];
        o.extend(pane.to_le_bytes());
        put_str(&mut o, name);
        send(&mut self.s, &o);
    }
}

#[test]
fn a_terminal_s_project_is_told_as_its_machine_has_it() {
    let s = Session::start();
    let repo = s.dir.join("calendula");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(repo.join("src")).unwrap();
    let mut a = Window::hello(&s.socket(), 1, false, CAN_SHARE | CAN_PROJECT);
    let t = a.spawn_in(&repo.join("src"));
    let told = a.told_of(t, Duration::from_secs(3)).expect("told");
    assert_eq!(told.split('\t').next(), Some("calendula"), "{told:?}");
}

#[test]
fn a_terminal_named_from_another_machine_is_kept_and_told() {
    let s = Session::start();
    let mut a = Window::hello(&s.socket(), 1, false, CAN_SHARE | CAN_PROJECT);
    let t = a.spawn();
    a.told_of(t, Duration::from_secs(3)).expect("told at first");
    a.names(t, "builds");
    let told = a.told_of(t, Duration::from_secs(3)).expect("told again");
    assert_eq!(told.split('\t').nth(11), Some("builds"), "{told:?}");
    // Kept as this machine's windows keep names.
    let file = std::fs::read_to_string(s.dir.join("state/jot/names")).unwrap_or_default();
    assert!(file.contains(&format!("\t{t}\tbuilds")), "{file:?}");
}

#[test]
fn a_window_from_before_projects_is_not_told() {
    let s = Session::start();
    let mut old = Window::hello(&s.socket(), 1, false, CAN_SHARE);
    let t = old.spawn();
    assert!(old.told_of(t, Duration::from_millis(1500)).is_none(), "it would not know the message");
}

// What a window on another machine says in its hello (FROM_AFAR), and the
// resize and size messages.
const FROM_AFAR: u32 = 16;
const RESIZE: u8 = 3;
const SIZE: u8 = 13;

impl Window {
    /// Its pane resized: `cols` by `rows`.
    fn resizes(&mut self, pane: u64, cols: u16, rows: u16) {
        let mut o = vec![RESIZE];
        o.extend(pane.to_le_bytes());
        o.extend(cols.to_le_bytes());
        o.extend(rows.to_le_bytes());
        o.extend(0u32.to_le_bytes());
        o.extend(0u32.to_le_bytes());
        send(&mut self.s, &o);
    }

    /// The size the session tells it a pane is, next, in `wait`.
    fn told_size(&mut self, pane: u64, wait: Duration) -> Option<(u16, u16)> {
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.checked_duration_since(Instant::now())?;
            let f = self.expect(SIZE, left)?;
            let mut i = In(&f[1..]);
            if i.u64() == pane {
                let cols = u16::from_le_bytes(i.take(2).try_into().unwrap());
                let rows = u16::from_le_bytes(i.take(2).try_into().unwrap());
                return Some((cols, rows));
            }
        }
    }
}

/// Reported: a terminal's font made larger in a window on another machine
/// (or its panes split there) resized the terminal under its own machine's
/// window. A window here gives it its size while one shows it; the one
/// from afar is told that size, to draw it at.
#[test]
fn a_window_from_afar_does_not_resize_a_terminal_a_window_here_shows() {
    let s = Session::start();
    let mut here = s.window(1, true);
    let t = here.spawn();
    here.says(&ws("w1", 10, 0, &one(t)));
    here.settles(A_WHILE);
    here.resizes(t, 100, 30);
    assert_eq!(here.told_size(t, Duration::from_secs(2)), Some((100, 30)), "its own size, told");
    let mut afar = Window::hello(&s.socket(), 2, false, CAN_SHARE | FROM_AFAR);
    afar.settles(A_WHILE);
    afar.resizes(t, 60, 14);
    assert_eq!(afar.told_size(t, Duration::from_secs(2)), Some((100, 30)), "told the size the window here gives");
    assert_eq!(here.told_size(t, Duration::from_millis(800)), None, "the window here is not told another size");
    // The window here still gives it its size.
    here.resizes(t, 90, 28);
    assert_eq!(afar.told_size(t, Duration::from_secs(2)), Some((90, 28)));
}

/// Alone (no window here shows it), a window from afar sizes it.
#[test]
fn a_window_from_afar_alone_sizes_the_terminal() {
    let s = Session::start();
    let mut afar = Window::hello(&s.socket(), 2, false, CAN_SHARE | FROM_AFAR);
    let t = afar.spawn();
    afar.says(&ws("w1", 10, 0, &one(t)));
    afar.settles(A_WHILE);
    afar.resizes(t, 60, 14);
    assert_eq!(afar.told_size(t, Duration::from_secs(2)), Some((60, 14)));
}
