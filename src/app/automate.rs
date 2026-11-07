//! What scripts and the `jterm` command ask of jterm, triggers on what
//! panes print, and the commands a workspace runs when it opens or is
//! gone to.

use super::*;
use crate::remote::Request;

/// A pattern looked for in what panes print, and what to do on it.
pub struct Trigger {
    pub pattern: regex::Regex,
    pub action: String,
    pub command: Option<String>,
}

/// Runs a shell command in the background, in `cwd`, with `env` set.
pub fn run_detached(command: &str, cwd: &std::path::Path, env: &[(&str, &str)]) {
    let mut c = std::process::Command::new("sh");
    c.arg("-c").arg(command).current_dir(cwd).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    for (k, v) in env {
        c.env(k, v);
    }
    let _ = c.spawn();
}

/// A desktop notice.
pub fn notify(title: &str, body: &str) {
    let _ = std::process::Command::new("notify-send").args(["-a", "jterm", title, body]).stdin(std::process::Stdio::null()).spawn();
}

/// What a pane printed as plain lines: escape sequences taken out.
fn plain(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.next() {
                Some('[') => {
                    while let Some(&n) = chars.peek() {
                        chars.next();
                        if ('@'..='~').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(n) = chars.next() {
                        if n == '\x07' || (n == '\x1b' && chars.peek() == Some(&'\\')) {
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if c == '\r' {
            continue;
        } else {
            out.push(c);
        }
    }
    out
}

impl App {
    pub(super) fn load_triggers(&mut self) {
        self.triggers = self
            .config
            .triggers
            .iter()
            .filter_map(|t| match regex::Regex::new(&t.pattern) {
                Ok(pattern) => Some(Trigger { pattern, action: t.action.clone(), command: t.command.clone() }),
                Err(e) => {
                    eprintln!("jterm: trigger {:?} not used: {e}", t.pattern);
                    None
                }
            })
            .collect();
    }

    /// What a pane printed, looked through for the triggers' patterns.
    pub(super) fn check_triggers(&mut self, id: PaneId, bytes: &[u8]) {
        if self.triggers.is_empty() {
            return;
        }
        let text = plain(bytes);
        let place = self.pane_name(id);
        let cwd = self.panes.get(&id).and_then(|p| p.pty.cwd()).unwrap_or_else(pty::home);
        let mut toasts: Vec<(String, String)> = Vec::new();
        for (i, t) in self.triggers.iter().enumerate() {
            let Some(line) = text.lines().find(|l| t.pattern.is_match(l)) else { continue };
            // Not more than once in three seconds for a pane.
            let key = (i, id);
            if self.fired.get(&key).is_some_and(|at| at.elapsed() < Duration::from_secs(3)) {
                continue;
            }
            self.fired.insert(key, Instant::now());
            let line = line.trim().to_string();
            match t.action.as_str() {
                "open" => {
                    let chars: Vec<char> = line.chars().collect();
                    if let Some((a, b)) = links::urls(&chars).first() {
                        let url: String = chars[*a..*b].iter().collect();
                        run_detached(&format!("xdg-open '{}'", url.replace('\'', "")), &cwd, &[]);
                    }
                }
                "run" => {
                    if let Some(cmd) = &t.command {
                        run_detached(cmd, &cwd, &[("JTERM_MATCH", &line), ("JTERM_PANE", &place)]);
                    }
                }
                _ => toasts.push((place.clone(), line.clone())),
            }
        }
        for (title, body) in toasts {
            self.toast(&title, &body, None, Some(id));
        }
    }

    /// A pane by where it is: workspace and pane, counted from 1, as 1.2.
    fn pane_name(&self, id: PaneId) -> String {
        for (wi, ws) in self.workspaces.iter().enumerate() {
            let mut ids = Vec::new();
            ws.root.panes(&mut ids);
            if let Some(n) = ids.iter().position(|p| *p == id) {
                return format!("{}.{}", wi + 1, n + 1);
            }
        }
        "roaming".into()
    }

    fn pane_named(&self, name: &str) -> Option<(usize, PaneId)> {
        if name == "focused" || name == "." {
            return self.focused().map(|id| (self.current, id));
        }
        let (w, p) = name.split_once('.').unwrap_or((name, "1"));
        let wi = w.parse::<usize>().ok()?.checked_sub(1)?;
        let mut ids = Vec::new();
        self.workspaces.get(wi)?.root.panes(&mut ids);
        Some((wi, *ids.get(p.parse::<usize>().ok()?.checked_sub(1)?)?))
    }

    /// The commands a workspace runs when it opens and when it is gone to.
    pub(super) fn run_hook(&self, wi: usize, opening: bool) {
        let Some(ws) = self.workspaces.get(wi) else { return };
        let Some(cmd) = (if opening { &ws.on_open } else { &ws.on_focus }) else { return };
        let mut ids = Vec::new();
        ws.root.panes(&mut ids);
        let cwd = ids.first().and_then(|id| self.panes.get(id)).and_then(|p| p.pty.cwd()).unwrap_or_else(pty::home);
        run_detached(cmd, &cwd, &[("JTERM_WORKSPACE", &ws.name)]);
    }

    /// Answers what the `jterm` command asked.
    pub(super) fn answer(&mut self, req: Request, event_loop: &ActiveEventLoop) {
        let args = &req.args;
        let reply = match args.first().map(String::as_str) {
            Some("list") => {
                let mut out = String::new();
                for (wi, ws) in self.workspaces.iter().enumerate() {
                    let mark = if wi == self.current { "  (here)" } else { "" };
                    out += &format!("{} {}{mark}\n", wi + 1, ws.name);
                    let mut ids = Vec::new();
                    ws.root.panes(&mut ids);
                    for (n, id) in ids.iter().enumerate() {
                        let p = &self.panes[id];
                        let cwd = p.pty.cwd().map(|c| persist::shorten(&c)).unwrap_or_default();
                        let focus = if *id == ws.focus { " *" } else { "" };
                        out += &format!("  {}.{}  {}  {}{focus}\n", wi + 1, n + 1, cwd, p.caption);
                    }
                }
                out
            }
            Some("open") => match args.get(1).map(|d| persist::expand(d)) {
                Some(dir) if dir.is_dir() => {
                    let name = folder_name(&dir);
                    match self.workspaces.iter().position(|w| w.name == name) {
                        Some(wi) => self.go_to(wi),
                        None => {
                            self.add_workspace(&WorkspaceSpec { cwd: Some(dir.display().to_string()), ..Default::default() });
                            self.go_to(self.workspaces.len() - 1);
                        }
                    }
                    format!("{}\n", self.current + 1)
                }
                _ => "jterm: open needs a folder that is there\n".into(),
            },
            Some("split") | Some("run") => {
                let down = args.iter().any(|a| a == "--down");
                let cmd: Vec<&str> = match args.iter().position(|a| a == "--") {
                    Some(i) => args[i + 1..].iter().map(String::as_str).collect(),
                    None if args[0] == "run" => args[1..].iter().map(String::as_str).collect(),
                    None => Vec::new(),
                };
                let cwd = self.focused().and_then(|f| self.panes.get(&f)).and_then(|p| p.pty.cwd()).unwrap_or_else(pty::home);
                self.split_with(if down { Dir::Down } else { Dir::Across }, Start { cmd: cmd.join(" "), cwd });
                format!("{}\n", self.focused().map(|f| self.pane_name(f)).unwrap_or_default())
            }
            Some("send") => match (args.get(1).and_then(|n| self.pane_named(n)), args.get(2)) {
                (Some((_, id)), Some(text)) => {
                    if let Some(p) = self.panes.get(&id) {
                        p.pty.write(text.replace('\n', "\r").as_bytes());
                    }
                    String::new()
                }
                _ => "jterm: send needs a pane (1.2) and text\n".into(),
            },
            Some("focus") => match args.get(1).and_then(|n| self.pane_named(n)) {
                Some((wi, id)) => {
                    self.go_to(wi);
                    self.set_focus(id);
                    self.relayout();
                    String::new()
                }
                None => "jterm: no pane by that name\n".into(),
            },
            Some("notify") => {
                self.toast("jterm", &args[1..].join(" "), None, None);
                String::new()
            }
            _ => format!("jterm: one of {} is needed\n", crate::remote::COMMANDS.join(", ")),
        };
        let _ = event_loop;
        let _ = req.reply.send(reply);
    }
}
