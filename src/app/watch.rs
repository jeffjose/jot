//! What a pane prints, watched as it comes: /tee writes it to a file as
//! well, from whenever it is asked, until stopped; /filter shows only the
//! lines of the running command with some text in them, until it ends.
//! Each is shown by a tag on the pane's caption, which a click stops.

use super::*;
use std::io::Write as _;

/// A pane's output written to a file too, as plain text.
pub struct Tee {
    pub path: PathBuf,
    file: std::fs::File,
    pub lines: usize,
    /// When its tag was last drawn, its count with it.
    shown_at: Instant,
    /// Everything the pane prints, until stopped; else one command's
    /// output, that ends it.
    pub all: bool,
    /// Not yet on: waiting for the next command to start.
    pub armed: bool,
}

impl Tee {
    /// What of these bytes goes to the file: all of them, or only what
    /// the command printed, from its start mark to its end mark. The
    /// second is true once the command ended.
    fn take<'a>(&mut self, bytes: &'a [u8]) -> (&'a [u8], bool) {
        if self.all {
            return (bytes, false);
        }
        let mut rest = bytes;
        if self.armed {
            let Some(at) = mark(rest, b"\x1b]133;C") else { return (&[], false) };
            let end = rest[at..].iter().position(|b| *b == 0x07).map_or(rest.len(), |e| at + e + 1);
            rest = &rest[end..];
            self.armed = false;
        }
        match mark(rest, b"\x1b]133;D") {
            Some(at) => (&rest[..at], true),
            None => (rest, false),
        }
    }
}

/// A pane's command's lines, only those with `text` in them shown.
pub struct Filter {
    pub text: String,
    /// Not yet on: waiting for the next command to start.
    pub armed: bool,
    /// A line not ended yet, held until it is.
    partial: Vec<u8>,
    pub hidden: usize,
}

/// Where in `bytes` a shell mark starts, if it is there.
fn mark(bytes: &[u8], which: &[u8]) -> Option<usize> {
    bytes.windows(which.len()).position(|w| w == which)
}

impl Filter {
    /// Whether a line of plain text is one to show: case aside, unless
    /// the text asked for has capitals (as a smart case search).
    fn keeps(&self, line: &str) -> bool {
        if self.text.chars().any(char::is_uppercase) {
            line.contains(&self.text)
        } else {
            line.to_lowercase().contains(&self.text)
        }
    }

    /// The bytes to show of those that came: the command's lines that
    /// match, whole; what is before it starts and after it ends, as it is.
    /// The second is true once the command ended, the filter with it.
    pub fn pass(&mut self, bytes: &[u8]) -> (Vec<u8>, bool) {
        let mut out = Vec::new();
        let mut rest = bytes;
        if self.armed {
            // Before the command starts, all as it is (the line typed).
            match mark(rest, b"\x1b]133;C") {
                Some(at) => {
                    let end = rest[at..].iter().position(|b| *b == 0x07).map_or(rest.len(), |e| at + e + 1);
                    out.extend_from_slice(&rest[..end]);
                    rest = &rest[end..];
                    self.armed = false;
                }
                None => return (rest.to_vec(), false),
            }
        }
        // The command ended (its status mark, before the next prompt): the
        // line held goes, and all from there is shown.
        let (body, tail) = match mark(rest, b"\x1b]133;D") {
            Some(at) => (&rest[..at], Some(&rest[at..])),
            None => (rest, None),
        };
        self.partial.extend_from_slice(body);
        while let Some(nl) = self.partial.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=nl).collect();
            if self.keeps(&automate::plain(&line)) {
                out.extend_from_slice(&line);
            } else {
                self.hidden += 1;
            }
        }
        match tail {
            Some(tail) => {
                let held = std::mem::take(&mut self.partial);
                if !held.is_empty() && self.keeps(&automate::plain(&held)) {
                    out.extend_from_slice(&held);
                }
                out.extend_from_slice(tail);
                (out, true)
            }
            None => (out, false),
        }
    }
}

impl App {
    /// What a pane printed, through its filter and to its tee, before its
    /// terminal has it.
    pub(super) fn watch_output(&mut self, id: PaneId, bytes: Vec<u8>) -> Vec<u8> {
        let Some(p) = self.panes.get_mut(&id) else { return bytes };
        if let Some(t) = &mut p.tee {
            let (part, ended) = t.take(&bytes);
            let text = automate::plain(part);
            t.lines += text.matches('\n').count();
            // Its count on the caption, kept up a few times a second.
            if t.shown_at.elapsed() > Duration::from_millis(250) {
                t.shown_at = Instant::now();
                p.full = true;
            }
            if t.file.write_all(text.as_bytes()).is_err() {
                let path = persist::shorten(&t.path);
                p.tee = None;
                self.toast("Tee stopped", &format!("Could not write to {path}"), Some(0xE5484D), Some(id));
                return bytes;
            }
            // Its command ended: so does the tee, saying where it went.
            if ended {
                self.stop_tee(id);
            }
        }
        let Some(p) = self.panes.get_mut(&id) else { return bytes };
        let Some(f) = &mut p.filter else { return bytes };
        let (shown, ended) = f.pass(&bytes);
        if ended {
            p.filter = None;
            p.full = true;
        }
        shown
    }

    /// /tee FILE: the running command's output written to the file as
    /// well, until it ends; at a prompt, the next command's. With --all,
    /// everything the pane prints, until stopped. With no file, a new one
    /// in /tmp/jterm named for the command; with one going, it stops.
    pub(super) fn tee(&mut self, id: PaneId, arg: &str) {
        let mut words: Vec<&str> = arg.split_whitespace().collect();
        let all = words.iter().any(|w| *w == "--all");
        words.retain(|w| *w != "--all");
        let file = words.join(" ");
        if file.is_empty() && !all && self.panes.get(&id).is_some_and(|p| p.tee.is_some()) {
            return self.stop_tee(id);
        }
        let running = self.panes.get(&id).is_some_and(|p| !p.shell.at_prompt);
        let cwd = self.panes.get(&id).and_then(|p| p.pty.cwd()).unwrap_or_else(pty::home);
        let path = if file.is_empty() {
            self.tee_file(id)
        } else {
            let p = persist::expand(&file);
            if p.is_absolute() { p } else { cwd.join(p) }
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => {
                if let Some(p) = self.panes.get_mut(&id) {
                    p.tee = Some(Tee { path: path.clone(), file, lines: 0, shown_at: Instant::now(), all, armed: !all && !running });
                    p.full = true;
                }
            }
            Err(e) => self.toast("Could not tee", &format!("{}: {e}", persist::shorten(&path)), Some(0xE5484D), Some(id)),
        }
        self.full = true;
        self.redraw();
    }

    /// A new file for a tee with none named: /tmp/jterm/COMMAND-TIME.log,
    /// the command as the line has it, its first word.
    fn tee_file(&self, id: PaneId) -> PathBuf {
        let word = self
            .panes
            .get(&id)
            .and_then(|p| p.shell.line.clone().or_else(|| p.shell.running.as_ref().map(|r| r.2.clone())))
            .and_then(|l| l.split_whitespace().next().map(str::to_string))
            .map(|w| w.rsplit('/').next().unwrap_or(&w).chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect::<String>())
            .filter(|w| !w.is_empty())
            .unwrap_or_else(|| "output".into());
        let stamp = std::process::Command::new("date").arg("+%Y%m%d-%H%M%S").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        std::env::temp_dir().join("jterm").join(format!("{word}-{stamp}.log"))
    }

    pub(super) fn stop_tee(&mut self, id: PaneId) {
        let Some(t) = self.panes.get_mut(&id).and_then(|p| p.tee.take()) else { return };
        if let Some(p) = self.panes.get_mut(&id) {
            p.full = true;
        }
        self.toast("Saved", &format!("{} lines · {}", t.lines, persist::shorten(&t.path)), None, Some(id));
        self.full = true;
        self.redraw();
    }

    /// /filter TEXT: only the running command's lines with the text in
    /// them shown, until it ends; at a prompt, the next command's.
    /// /filter alone: stopped.
    pub(super) fn filter(&mut self, id: PaneId, arg: &str) {
        let text = arg.trim();
        if text.is_empty() {
            return self.stop_filter(id);
        }
        let running = self.panes.get(&id).is_some_and(|p| !p.shell.at_prompt);
        if let Some(p) = self.panes.get_mut(&id) {
            p.filter = Some(Filter { text: text.to_string(), armed: !running, partial: Vec::new(), hidden: 0 });
            p.full = true;
        }
        self.full = true;
        self.redraw();
    }

    pub(super) fn stop_filter(&mut self, id: PaneId) {
        let Some(p) = self.panes.get_mut(&id) else { return };
        let Some(mut f) = p.filter.take() else { return };
        // What was held of a line goes on, matching or not, as the
        // filter is no more.
        let held = std::mem::take(&mut f.partial);
        if !held.is_empty() {
            p.term.write(&held);
        }
        p.full = true;
        self.full = true;
        self.redraw();
    }
}

/// A tag on a pane's caption that can be clicked.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tag {
    /// Typing into several panes: takes this one in or out.
    Multi,
    /// Writing to a file: stops it.
    Tee,
    /// The × on a TEE tag: stops it.
    TeeStop,
    /// Lines filtered: stops it.
    Filter,
    /// To be told when the command ends: no longer.
    Notify,
    /// The restart button, on a pane running a command.
    Restart,
    /// The focus button: this pane alone, or all of them again.
    Focus,
}

impl App {
    /// /notify: told when the pane's command ends, or its next one if none
    /// is running; or not, any more.
    pub(super) fn set_notify(&mut self, id: PaneId, on: bool) {
        if let Some(p) = self.panes.get_mut(&id) {
            p.shell.notify = on;
            p.full = true;
        }
        self.full = true;
        self.redraw();
    }
}
