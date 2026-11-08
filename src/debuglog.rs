//! With --debug (or JOT_DEBUG=1 in its environment), jot writes what it
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
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("debug.log")) {
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
