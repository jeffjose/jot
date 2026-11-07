//! /info: what a pane is running and what it is, read from /proc. At a
//! prompt it is printed into the terminal as if a command had printed it;
//! in a pane's command line, it is shown there.

use super::*;

/// Lines of text printed into a pane's terminal, not by its program.
pub struct Print {
    pub text: String,
    /// When the shell was asked to rub out what was typed: the text goes
    /// in once it has.
    pub asked: Instant,
}

/// A process's name and arguments, as it was started.
fn cmdline(pid: i32) -> Option<String> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let words: Vec<String> = raw.split(|b| *b == 0).filter(|w| !w.is_empty()).map(|w| String::from_utf8_lossy(w).into_owned()).collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// A field of /proc/PID/status, as it is written there.
fn status(pid: i32, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix(':').map(|v| v.trim().to_string()))
}

/// How long ago a process started, from its start time in /proc.
fn age(pid: i32) -> Option<Duration> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Past the name, which may hold spaces, in its parentheses.
    let rest = &stat[stat.rfind(')')? + 2..];
    let start: f64 = rest.split(' ').nth(19)?.parse().ok()?;
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    let uptime: f64 = std::fs::read_to_string("/proc/uptime").ok()?.split(' ').next()?.parse().ok()?;
    Some(Duration::from_secs_f64((uptime - start / ticks).max(0.0)))
}

/// Memory in kB as /proc writes it, more readably.
fn memory(kb: &str) -> String {
    let n: f64 = kb.trim_end_matches(" kB").trim().parse().unwrap_or(0.0);
    if n >= 1024.0 * 1024.0 {
        format!("{:.1} GB", n / 1024.0 / 1024.0)
    } else if n >= 1024.0 {
        format!("{:.0} MB", n / 1024.0)
    } else {
        format!("{n:.0} kB")
    }
}

impl App {
    /// What /info says of a pane, as rows of a name and what it is.
    pub(super) fn pane_info(&self, id: PaneId) -> Vec<(String, String)> {
        let Some(p) = self.panes.get(&id) else { return Vec::new() };
        let mut rows: Vec<(String, String)> = Vec::new();
        let shell = p.pty.pid;
        let pgrp = unsafe { libc::tcgetpgrp(p.pty.fd) };
        if pgrp > 0 && pgrp != shell {
            rows.push(("running".into(), cmdline(pgrp).unwrap_or_else(|| "?".into())));
            rows.push(("pid".into(), format!("{pgrp} (its process group)")));
            let took = p.shell.running.as_ref().map(|(at, _, _)| at.elapsed()).or_else(|| age(pgrp));
            if let Some(t) = took {
                rows.push(("for".into(), commands::human(t)));
            }
            if let Some(state) = status(pgrp, "State") {
                rows.push(("state".into(), state));
            }
            if let Some(rss) = status(pgrp, "VmRSS") {
                rows.push(("memory".into(), memory(&rss)));
            }
        } else {
            rows.push(("running".into(), "nothing, the shell is waiting".into()));
        }
        let name = std::fs::read_to_string(format!("/proc/{shell}/comm")).map(|s| s.trim().to_string()).unwrap_or_default();
        rows.push(("shell".into(), format!("{name} · pid {shell}")));
        if let Some(cwd) = p.pty.cwd() {
            rows.push(("folder".into(), persist::shorten(&cwd)));
        }
        let tty = unsafe { libc::ptsname(p.pty.fd) };
        let tty = if tty.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(tty) }.to_string_lossy().into_owned() };
        rows.push(("terminal".into(), format!("{tty} · {}×{}", p.cols, p.rows)));
        if let Some(b) = p.shell.blocks.last().filter(|b| !b.command.is_empty()) {
            let how = match b.exit {
                Some(0) => "ok".to_string(),
                Some(n) => format!("exit {n}"),
                None => "ended".to_string(),
            };
            rows.push(("last".into(), format!("{} · {how} · {}", b.command, commands::human(b.took))));
        }
        rows
    }

    /// The rows as text, the names in a column.
    fn info_text(rows: &[(String, String)], color: bool) -> Vec<String> {
        let w = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
        rows.iter()
            .map(|(k, v)| if color { format!("\x1b[2m{k:>w$}\x1b[0m  {v}") } else { format!("{k:>w$}  {v}") })
            .collect()
    }

    /// /info at a prompt: printed into the terminal under `/info`, as a
    /// command's output would be, and the shell given a fresh prompt.
    pub(super) fn print_info(&mut self, id: PaneId) {
        let lines = Self::info_text(&self.pane_info(id), true);
        let text = format!("/info\r\n{}", lines.join("\r\n"));
        if let Some(p) = self.panes.get_mut(&id) {
            p.print = Some(info::Print { text, asked: Instant::now() });
        }
    }

    /// /info in a pane's command line: shown there.
    pub(super) fn bar_info(&mut self, id: PaneId) {
        let text = Self::info_text(&self.pane_info(id), false).join("\n");
        if let Some(p) = self.panes.get_mut(&id) {
            p.shell.ran = Some(("/info".into(), text, Some(0)));
        }
    }

    /// Text waiting to be printed into a pane, put in once the shell has
    /// rubbed out what was typed; then an empty Enter to the shell, for
    /// a fresh prompt under it. True while any still waits.
    pub(super) fn print_tick(&mut self) -> bool {
        let mut waiting = false;
        for p in self.panes.values_mut() {
            let Some(pr) = &p.print else { continue };
            if pr.asked.elapsed() < Duration::from_millis(80) {
                waiting = true;
                continue;
            }
            let text = p.print.take().map(|pr| pr.text).unwrap_or_default();
            p.term.write(text.as_bytes());
            p.pty.write(b"\r");
            p.full = true;
            self.full = true;
        }
        if self.full {
            self.redraw();
        }
        waiting
    }
}
