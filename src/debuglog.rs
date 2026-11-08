//! With --debug, JOT_DEBUG=1 in its environment, or the switch on (`jot
//! debug on`, or Settings, Debug), jot writes what it
//! does to ~/.local/state/jot/debug.log: what happened as it happens
//! (a link lost and found, a machine's listing changed, a terminal made
//! another size), and once a second a count of what it drew and why, and
//! of what its sessions told it. Without, nothing is kept or written.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Write;

thread_local! {
    static COUNTS: RefCell<BTreeMap<&'static str, u32>> = RefCell::new(BTreeMap::new());
}

fn stamp() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let (s, ms) = (now.as_secs(), now.subsec_millis());
    format!("{:02}:{:02}:{:02}.{ms:03}", s / 3600 % 24, s / 60 % 60, s % 60)
}

fn write(line: &str) {
    let dir = crate::persist::state_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log_path()) {
        let _ = writeln!(f, "{} [{}] {line}", stamp(), std::process::id());
    }
}

/// A line now, when debugging.
pub fn log(line: impl AsRef<str>) {
    if crate::debug() {
        write(line.as_ref());
    }
}

/// One more of `what`, told in the next summary.
pub fn count(what: &'static str) {
    if crate::debug() {
        COUNTS.with(|c| *c.borrow_mut().entry(what).or_insert(0) += 1);
    }
}

/// The counts since the last summary, as one line (none, when nothing was
/// counted); called once a second.
pub fn summary() {
    if !crate::debug() {
        return;
    }
    let line = COUNTS.with(|c| {
        let mut c = c.borrow_mut();
        let line = c.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ");
        c.clear();
        line
    });
    if !line.is_empty() {
        write(&format!("per second: {line}"));
    }
}

/// The file that, while it is there, has every jot on this machine (each
/// window, the quake window, the session) debugging: made and taken away
/// by `jot debug on|off` and the switch in Settings, so that logging is
/// turned on when something goes wrong and off after, with no restart.
pub fn switch_path() -> std::path::PathBuf {
    crate::persist::state_dir().join("debug-on")
}

/// Where the log goes.
pub fn log_path() -> std::path::PathBuf {
    crate::persist::state_dir().join("debug.log")
}

static ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// When the switch was last looked for, in ms; 0 for never.
static LOOKED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Whether the switch is on: looked for at most once a second, and a line
/// written when this process sees it turned on or off.
pub fn switched_on() -> bool {
    use std::sync::atomic::Ordering;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
    let last = LOOKED.load(Ordering::Relaxed);
    if last != 0 && now.saturating_sub(last) < 1000 {
        return ON.load(Ordering::Relaxed);
    }
    LOOKED.store(now, Ordering::Relaxed);
    let on = switch_path().exists();
    if ON.swap(on, Ordering::Relaxed) != on {
        let args: Vec<String> = std::env::args().skip(1).collect();
        write(&format!("debug {} ({} {})", if on { "on" } else { "off" }, env!("CARGO_PKG_VERSION"), if args.is_empty() { "window".into() } else { args.join(" ") }));
    }
    on
}

/// The switch turned on or off.
pub fn set(on: bool) -> std::io::Result<()> {
    // Looked for again at the next asking, here at once.
    LOOKED.store(0, std::sync::atomic::Ordering::Relaxed);
    let path = switch_path();
    if on {
        std::fs::create_dir_all(crate::persist::state_dir())?;
        std::fs::write(&path, b"")
    } else {
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

/// `jot debug [on|off]`: the switch turned, or said.
pub fn command(args: &[String]) -> i32 {
    let want = match args.get(1).map(String::as_str) {
        None | Some("status") => None,
        Some("on") => Some(true),
        Some("off") => Some(false),
        Some("toggle") => Some(!switch_path().exists()),
        Some(other) => {
            eprintln!("jot debug: on, off, toggle or status, not {other}");
            return 2;
        }
    };
    if let Some(on) = want {
        if let Err(e) = set(on) {
            eprintln!("jot debug: {}: {e}", switch_path().display());
            return 1;
        }
    }
    let on = switch_path().exists();
    println!("debug {}: {}", if on { "on" } else { "off" }, crate::persist::shorten(&log_path()));
    0
}
