//! What scripts and the `jot` command ask of jot, triggers on what
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
    let _ = std::process::Command::new("notify-send").args(["-a", "jot", title, body]).stdin(std::process::Stdio::null()).spawn();
}

/// What a pane printed as plain lines: escape sequences taken out.
pub(crate) fn plain(bytes: &[u8]) -> String {
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
        } else if c == '\x08' {
            // A backspace, as a line editor rubs out: the character before goes.
            if !out.ends_with('\n') {
                out.pop();
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
                    eprintln!("jot: trigger {:?} not used: {e}", t.pattern);
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
                        run_detached(cmd, &cwd, &[("JOT_MATCH", &line), ("JOT_PANE", &place)]);
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
        run_detached(cmd, &cwd, &[("JOT_WORKSPACE", &ws.name)]);
    }

    /// Answers what the `jot` command asked.
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
                        let Some(p) = self.panes.get(id) else {
                            out += &format!("  {}.{}  (no pane {id:x})\n", wi + 1, n + 1);
                            continue;
                        };
                        let cwd = p.pty.cwd().map(|c| persist::shorten(&c)).unwrap_or_default();
                        let focus = if *id == ws.focus { " *" } else { "" };
                        out += &format!("  {}.{}  {}  {}{focus}\n", wi + 1, n + 1, cwd, p.caption);
                    }
                }
                // -v: what is behind it, for working out what went wrong.
                if args.iter().any(|a| a == "-v") {
                    out += &self.debug_state();
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
                _ => "jot: open needs a folder that is there\n".into(),
            },
            Some("split") | Some("run") => {
                let down = args.iter().any(|a| a == "--down");
                let cmd: Vec<&str> = match args.iter().position(|a| a == "--") {
                    Some(i) => args[i + 1..].iter().map(String::as_str).collect(),
                    None if args[0] == "run" => args[1..].iter().map(String::as_str).collect(),
                    None => Vec::new(),
                };
                let cwd = self.focused().and_then(|f| self.panes.get(&f)).and_then(|p| p.pty.cwd()).unwrap_or_else(pty::home);
                self.split_with(if down { Dir::Down } else { Dir::Across }, Start { cmd: cmd.join(" "), cwd, pane: None, host: None });
                format!("{}\n", self.focused().map(|f| self.pane_name(f)).unwrap_or_default())
            }
            Some("send") => match (args.get(1).and_then(|n| self.pane_named(n)), args.get(2)) {
                (Some((_, id)), Some(text)) => {
                    if let Some(p) = self.panes.get(&id) {
                        p.pty.write(text.replace('\n', "\r").as_bytes());
                    }
                    String::new()
                }
                _ => "jot: send needs a pane (1.2) and text\n".into(),
            },
            Some("focus") => match args.get(1).and_then(|n| self.pane_named(n)) {
                Some((wi, id)) => {
                    self.go_to(wi);
                    self.set_focus(id);
                    self.relayout();
                    String::new()
                }
                None => "jot: no pane by that name\n".into(),
            },
            // `jot click M:T`: what a click on terminal T of machine M's row in
            // the sidebar does (taken into the pane you are in, or gone to),
            // for tests to do it as a hand does, where it is drawn aside.
            Some("click") => {
                let at = args.get(1).and_then(|a| a.split_once(':')).and_then(|(m, t)| Some((m.parse::<usize>().ok()?, t.parse::<u64>().ok()?)));
                match at {
                    Some((k, sid)) => {
                        let shown = self.views(crate::mux::app_id(k, sid)).into_iter().find(|v| self.ws().root.contains(*v)).or_else(|| self.views(crate::mux::app_id(k, sid)).first().copied());
                        let hit = match shown {
                            Some(id) => sidebar::SideHit::Pane(id),
                            None => sidebar::SideHit::Free(k, sid),
                        };
                        self.side_row_click(hit);
                        String::new()
                    }
                    None => "jot: click needs a terminal, as machine:number (jot list -v)\n".into(),
                }
            }
            Some("tree") => self.tree(),
            Some("notify") => {
                self.toast("jot", &args[1..].join(" "), None, None);
                String::new()
            }
            // `jot --quake`, to the quake window: shown, or hidden.
            Some("toggle") if crate::quake_mode() => {
                self.quake_toggle();
                String::new()
            }
            // `jot restart`, to the quake window: itself again as the jot
            // installed now (`exe`), in this same process. Its workspaces
            // told to the session first; its connections and its socket
            // close with the exec, and the new one takes them up, and its
            // terminals, as the quake window coming back. Hidden, it stays
            // hidden.
            Some("restart") if crate::quake_mode() => match args.get(1).filter(|e| !e.is_empty()) {
                Some(exe) => {
                    if super::quake::wayland::restart(exe, !self.quake_shown, &self.config) {
                        self.save_scrollback();
                        self.save();
                        let _ = req.reply.send("restarting\n".into());
                        let _ = std::fs::remove_file(crate::remote::quake_socket_path());
                        event_loop.exit();
                        return;
                    }
                    let mut argv = vec!["--quake-window"];
                    if !self.quake_shown {
                        argv.push("--quake-hidden");
                    }
                    self.exec_again(exe, &argv);
                    "restarting\n".into()
                }
                None => "jot: restart needs the jot to start\n".into(),
            },
            _ => format!("jot: one of {} is needed\n", crate::remote::COMMANDS.join(", ")),
        };
        let _ = event_loop;
        let _ = req.reply.send(reply);
    }
}

impl App {
    /// `jot restart --all`, told by the session: this window closed and
    /// opened again as the jot at `exe`, as the same window (--again, its
    /// name in the session), which takes back its workspaces, terminals and
    /// place.
    pub(super) fn reopen_as(&mut self, exe: &str) {
        if crate::quake_mode() {
            return;
        }
        crate::debuglog::log(format!("reopening as {exe}"));
        let id = format!("{:x}", crate::mux::window_id());
        // A window on another machine's session (jot ssh HOST) opens on it again.
        let host = self.machines.first().and_then(|m| m.host.clone());
        let mut args: Vec<&str> = host.as_deref().map(|h| vec!["ssh", h]).unwrap_or_default();
        args.extend(["--again", &id]);
        self.exec_again(exe, &args);
    }

    /// This process become the jot at `exe` again, with `args` (exec): the
    /// workspaces told to the session first; its connections close with the
    /// exec, and the new one takes them up as this window coming back.
    fn exec_again(&mut self, exe: &str, args: &[&str]) {
        self.save();
        let exe = std::ffi::CString::new(exe).unwrap_or_default();
        let argv: Vec<std::ffi::CString> = std::iter::once("jot").chain(args.iter().copied()).filter_map(|a| std::ffi::CString::new(a).ok()).collect();
        // After the answer has gone, and what was saved with it.
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            let mut ptrs: Vec<*const libc::c_char> = argv.iter().map(|a| a.as_ptr()).collect();
            ptrs.push(std::ptr::null());
            unsafe { libc::execv(exe.as_ptr(), ptrs.as_ptr()) };
            eprintln!("jot: could not start again: {}", std::io::Error::last_os_error());
        });
    }
}

impl App {
    /// The palette's Restart jot: `jot restart --all`, run apart from the
    /// window (which it closes and opens again), as the jot installed now.
    pub(super) fn restart_all(&mut self) {
        let exe = std::env::current_exe().map(|e| PathBuf::from(e.to_string_lossy().trim_end_matches(" (deleted)"))).unwrap_or_else(|_| PathBuf::from("jot"));
        crate::debuglog::log(format!("restart --all as {}", exe.display()));
        self.save();
        super::open_detached(&exe, &["restart".as_ref(), "--all".as_ref()]);
    }
}
