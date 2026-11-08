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
        if p.pty.remote() {
            let host = p.pty.mux.host.clone().unwrap_or_default();
            let mut rows = vec![("machine".to_string(), host)];
            if let Some(cwd) = p.pty.cwd() {
                rows.push(("folder".into(), cwd.display().to_string()));
            }
            rows.push(("running".into(), p.pty.foreground().unwrap_or_default()));
            return rows;
        }
        let mut rows: Vec<(String, String)> = Vec::new();
        let shell = p.pty.pid;
        let pgrp = p.pty.foreground_group();
        if pgrp > 0 && pgrp != shell {
            rows.push(("running".into(), cmdline(pgrp).unwrap_or_else(|| "?".into())));
            rows.push(("pid".into(), format!("{pgrp} (its process group)")));
            let took = p.shell.running.as_ref().map(|(at, _, _)| at.elapsed()).or_else(|| age(pgrp));
            if let Some(t) = took {
                rows.push(("elapsed".into(), commands::human(t)));
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
        let tty = std::fs::read_link(format!("/proc/{}/fd/0", p.pty.pid)).map(|t| t.display().to_string()).unwrap_or_default();
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

    /// What /info says, in four: the pane, its workspace, this window (the
    /// jot it is, and the machines it reaches), and the session's server.
    pub(super) fn info_sections(&self, id: PaneId) -> Vec<(&'static str, Vec<(String, String)>)> {
        vec![("pane", self.pane_info(id)), ("workspace", self.workspace_info(id)), ("window", self.window_info()), ("session", self.session_info())]
    }

    /// The pane's workspace: its name, its panes, and where they run.
    fn workspace_info(&self, id: PaneId) -> Vec<(String, String)> {
        let Some(ws) = self.workspaces.iter().find(|w| {
            let mut ids = Vec::new();
            w.root.panes(&mut ids);
            ids.contains(&id)
        }) else {
            return vec![("name".into(), "none (a pane on its own)".into())];
        };
        let mut ids = Vec::new();
        ws.root.panes(&mut ids);
        let panes = ids.iter().filter(|p| !ws.spaces.contains(p)).count();
        let spaces = ids.len() - panes;
        let mut rows = vec![("name".to_string(), ws.name.clone())];
        rows.push(("panes".into(), if spaces > 0 { format!("{panes}, and {spaces} empty") } else { panes.to_string() }));
        let mut machines: Vec<String> = ids.iter().filter(|p| !ws.spaces.contains(p)).map(|p| self.machine_name(self.machine_of(*p))).collect();
        machines.dedup();
        machines.sort();
        machines.dedup();
        rows.push(("on".into(), machines.join(", ")));
        if ws.broadcast {
            rows.push(("typing".into(), "into every pane".into()));
        }
        rows
    }

    /// This window: the jot it runs, and the machines whose panes it shows.
    fn window_info(&self) -> Vec<(String, String)> {
        let panes = self.workspaces.iter().map(|w| {
            let mut ids = Vec::new();
            w.root.panes(&mut ids);
            ids.iter().filter(|p| !w.spaces.contains(p)).count()
        });
        let panes: usize = panes.sum();
        let mut rows = vec![("version".to_string(), format!("{} · pid {}", crate::version(), std::process::id()))];
        rows.push(("shows".into(), format!("{} · {}", count(self.workspaces.len(), "workspace"), count(panes, "pane"))));
        let machines: Vec<String> = self
            .machines
            .iter()
            .enumerate()
            .map(|(k, m)| {
                let state = if k == 0 && m.host.is_none() { String::new() } else if m.client.connected() { " (connected)".into() } else { " (reaching it)".into() };
                format!("{}{state}", self.machine_name(k))
            })
            .collect();
        rows.push(("machines".into(), machines.join(", ")));
        rows
    }

    /// The session's server, on this machine: the jot it runs (another
    /// than the window's after an update, until jot restart), how long it
    /// has, and all it holds, every window's.
    fn session_info(&self) -> Vec<(String, String)> {
        if let Some(host) = &self.mux.host {
            return vec![("machine".into(), host.clone())];
        }
        let mut rows = Vec::new();
        let pid = crate::mux::server_pid(&crate::mux::socket());
        match crate::mux::peek() {
            Ok(crate::mux::ToClient::Info { panes, names, version, windows, .. }) => {
                let said = if version.is_empty() {
                    "a jot older than jot restart (jot kill once, to be done with it)".to_string()
                } else if version != crate::version() {
                    format!("{version} · not this window's, jot restart swaps it")
                } else {
                    version
                };
                rows.push(("server".into(), said));
                if let Some(pid) = pid {
                    let up = age(pid).map(|d| format!(" · up {}", commands::human(d))).unwrap_or_default();
                    rows.push(("pid".into(), format!("{pid}{up}")));
                }
                let n = names.split(", ").filter(|n| !n.is_empty()).count();
                rows.push(("holds".into(), format!("{} · {}", count(panes as usize, "pane"), count(n, "workspace"))));
                rows.push(("windows".into(), if windows == 0 { "?".into() } else { windows.to_string() }));
            }
            _ => rows.push(("server".into(), "not answering".into())),
        }
        rows.push(("socket".into(), persist::shorten(&crate::mux::socket())));
        rows
    }

    /// A machine as /info names it: its host, or this one's name.
    fn machine_name(&self, k: usize) -> String {
        match self.machines.get(k).and_then(|m| m.host.clone()) {
            Some(h) => h,
            None => std::fs::read_to_string("/proc/sys/kernel/hostname").map(|s| s.trim().to_string()).unwrap_or_else(|_| "this machine".into()),
        }
    }

    /// The sections as text: each under its heading, the names in one
    /// column throughout.
    fn info_text(sections: &[(&str, Vec<(String, String)>)], color: bool) -> Vec<String> {
        let w = sections.iter().flat_map(|(_, rows)| rows.iter().map(|(k, _)| k.chars().count())).max().unwrap_or(0);
        let mut out = Vec::new();
        for (i, (head, rows)) in sections.iter().enumerate() {
            if i > 0 {
                out.push(String::new());
            }
            out.push(if color { format!("\x1b[1m{head}\x1b[0m") } else { head.to_string() });
            for (k, v) in rows {
                out.push(if color { format!("\x1b[2m  {k:>w$}\x1b[0m  {v}") } else { format!("  {k:>w$}  {v}") });
            }
        }
        out
    }

    /// /info at a prompt: printed into the terminal under `/info`, as a
    /// command's output would be, and the shell given a fresh prompt.
    pub(super) fn print_info(&mut self, id: PaneId) {
        let lines = Self::info_text(&self.info_sections(id), true);
        let text = format!("/info\r\n{}", lines.join("\r\n"));
        if let Some(p) = self.panes.get_mut(&id) {
            p.print = Some(info::Print { text, asked: Instant::now() });
        }
    }

    /// /info in a pane's command line: shown there.
    pub(super) fn bar_info(&mut self, id: PaneId) {
        let text = Self::info_text(&self.info_sections(id), false).join("\n");
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

/// The processes started by `pid`, each task's children.
fn children(pid: i32) -> Vec<i32> {
    let mut kids = Vec::new();
    if let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) {
        for t in tasks.flatten() {
            if let Ok(text) = std::fs::read_to_string(t.path().join("children")) {
                kids.extend(text.split_whitespace().filter_map(|k| k.parse::<i32>().ok()));
            }
        }
    }
    kids.sort_unstable();
    kids.dedup();
    kids
}

/// A process and all it started, each with how deep it is.
fn tree(root: i32) -> Vec<(usize, i32)> {
    let mut out = Vec::new();
    let mut stack = vec![(0, root)];
    while let Some((depth, pid)) = stack.pop() {
        out.push((depth, pid));
        for kid in children(pid).into_iter().rev() {
            stack.push((depth + 1, kid));
        }
    }
    out
}

/// A process's open files and sockets: each descriptor's number and what
/// it points at.
fn fds(pid: i32) -> Vec<(u32, String)> {
    let Ok(dir) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { return Vec::new() };
    dir.flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().parse::<u32>().ok()?;
            let to = std::fs::read_link(e.path()).ok()?;
            Some((n, to.to_string_lossy().into_owned()))
        })
        .collect()
}

/// Listening TCP sockets, by inode: the port, and whether only this
/// machine can reach it.
fn listening() -> std::collections::HashMap<u64, (u16, bool)> {
    let mut out = std::collections::HashMap::new();
    for file in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(text) = std::fs::read_to_string(file) else { continue };
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 10 || f[3] != "0A" {
                continue;
            }
            let Some((addr, port)) = f[1].split_once(':') else { continue };
            let Ok(port) = u16::from_str_radix(port, 16) else { continue };
            let local = addr == "0100007F" || addr == "00000000000000000000000001000000";
            if let Ok(inode) = f[9].parse::<u64>() {
                out.insert(inode, (port, local));
            }
        }
    }
    out
}

impl App {
    /// The process a report is about: the running job, else the shell.
    fn report_root(&self, id: PaneId) -> Option<(i32, bool)> {
        let p = self.panes.get(&id)?;
        let pgrp = p.pty.foreground_group();
        Some(if pgrp > 0 && pgrp != p.pty.pid { (pgrp, true) } else { (p.pty.pid, false) })
    }

    /// /tree: the job and every process it started, with their memory.
    pub(super) fn report_tree(&self, id: PaneId) -> Vec<String> {
        if self.panes.get(&id).is_some_and(|p| p.pty.remote()) {
            return vec!["Only for panes on this machine, for now.".into()];
        }
        let Some((root, _)) = self.report_root(id) else { return Vec::new() };
        tree(root)
            .into_iter()
            .map(|(depth, pid)| {
                let cmd = cmdline(pid).unwrap_or_else(|| "?".into());
                let mem = status(pid, "VmRSS").map(|m| memory(&m)).unwrap_or_default();
                format!("{}{pid:<8} {mem:>7}  {cmd}", "  ".repeat(depth))
            })
            .collect()
    }

    /// /ports: what the job's processes listen on, as links.
    pub(super) fn report_ports(&self, id: PaneId) -> Vec<String> {
        if self.panes.get(&id).is_some_and(|p| p.pty.remote()) {
            return vec!["Only for panes on this machine, for now.".into()];
        }
        let Some((root, _)) = self.report_root(id) else { return Vec::new() };
        let sockets = listening();
        let mut out = Vec::new();
        for (_, pid) in tree(root) {
            for (_, to) in fds(pid) {
                let Some(inode) = to.strip_prefix("socket:[").and_then(|s| s.strip_suffix(']')).and_then(|s| s.parse::<u64>().ok()) else { continue };
                if let Some((port, local)) = sockets.get(&inode) {
                    let name = std::fs::read_to_string(format!("/proc/{pid}/comm")).map(|s| s.trim().to_string()).unwrap_or_default();
                    let reach = if *local { "this machine" } else { "the network" };
                    let line = format!("http://localhost:{port}  {name} · pid {pid} · open to {reach}");
                    if !out.contains(&line) {
                        out.push(line);
                    }
                }
            }
        }
        if out.is_empty() {
            out.push("not listening on any port".into());
        }
        out
    }

    /// /files: the files the job's processes have open, and how.
    pub(super) fn report_files(&self, id: PaneId) -> Vec<String> {
        if self.panes.get(&id).is_some_and(|p| p.pty.remote()) {
            return vec!["Only for panes on this machine, for now.".into()];
        }
        let Some((root, _)) = self.report_root(id) else { return Vec::new() };
        let cwd = self.panes.get(&id).and_then(|p| p.pty.cwd());
        let mut out = Vec::new();
        for (_, pid) in tree(root) {
            for (n, to) in fds(pid) {
                if !to.starts_with('/') || to.starts_with("/dev/") || to.starts_with("/proc/") {
                    continue;
                }
                // How it was opened, from the flags /proc keeps.
                let flags = std::fs::read_to_string(format!("/proc/{pid}/fdinfo/{n}")).ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("flags:").map(|v| v.trim().to_string()))).and_then(|v| u32::from_str_radix(&v, 8).ok()).unwrap_or(0);
                let how = match flags & 3 {
                    1 => "write",
                    2 => "read+write",
                    _ => "read",
                };
                let shown = cwd.as_ref().and_then(|c| std::path::Path::new(&to).strip_prefix(c).ok()).map(|r| r.display().to_string()).unwrap_or_else(|| persist::shorten(std::path::Path::new(&to)));
                let line = format!("{how:<10} {shown}");
                if !out.contains(&line) {
                    out.push(line);
                }
            }
        }
        if out.is_empty() {
            out.push("no files open".into());
        }
        out
    }

    /// A report printed at a prompt, under the command that asked for it.
    pub(super) fn print_report(&mut self, id: PaneId, title: &str, lines: Vec<String>) {
        let text = format!("{title}\r\n{}", lines.join("\r\n"));
        if let Some(p) = self.panes.get_mut(&id) {
            p.print = Some(info::Print { text, asked: Instant::now() });
        }
    }

    /// A report in the pane's command line.
    pub(super) fn bar_report(&mut self, id: PaneId, title: &str, lines: Vec<String>) {
        if let Some(p) = self.panes.get_mut(&id) {
            p.shell.ran = Some((title.into(), lines.join("\n"), Some(0)));
        }
    }
}

/// "1 pane", "3 panes".
fn count(n: usize, what: &str) -> String {
    if n == 1 { format!("1 {what}") } else { format!("{n} {what}s") }
}
