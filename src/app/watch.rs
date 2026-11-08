//! What a pane prints, watched as it comes: /tee writes it to a file as
//! well, from whenever it is asked, until stopped; /filter shows only the
//! lines of the running command with some text in them, until it ends.
//! Each is shown by a tag on the pane's caption, which a click stops.

use super::*;

/// A pane's output written to a file too, as plain text: by the session,
/// so that it goes on with no window. This is what the caption shows.
pub struct Tee {
    pub path: PathBuf,
    pub lines: usize,
    /// Not yet on: waiting for the next command to start.
    pub armed: bool,
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
    /// in /tmp/jot named for the command; with one going, it stops.
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
        // The session writes it; a file it cannot open comes back as an end.
        let armed = !all && !running;
        if let Some(p) = self.panes.get_mut(&id) {
            p.pty.mux.tee(p.pty.pane, crate::mux::TeeState { path: path.display().to_string(), lines: 0, all, armed });
            p.tee = Some(Tee { path, lines: 0, armed });
            p.full = true;
        }
        self.full = true;
        self.redraw();
    }

    /// A new file for a tee with none named: /tmp/jot/COMMAND-TIME.log,
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
        std::env::temp_dir().join("jot").join(format!("{word}-{stamp}.log"))
    }

    /// A tee stopped: the session told, which says where it went when it
    /// has.
    pub(super) fn stop_tee(&mut self, id: PaneId) {
        if let Some(p) = self.panes.get(&id).filter(|p| p.tee.is_some()) {
            p.pty.mux.tee_stop(p.pty.pane);
        }
    }

    /// The session ended a tee: stopped, its command done, or the file
    /// failed. Where it went said, or why not.
    pub(super) fn tee_ended(&mut self, id: PaneId, lines: u64, error: String) {
        let Some(t) = self.panes.get_mut(&id).and_then(|p| p.tee.take()) else { return };
        if let Some(p) = self.panes.get_mut(&id) {
            p.full = true;
        }
        let path = persist::shorten(&t.path);
        if error.is_empty() {
            self.toast("Saved", &format!("{lines} lines · {path}"), None, Some(id));
        } else {
            self.toast("Tee stopped", &format!("{path}: {error}"), Some(0xE5484D), Some(id));
        }
        self.full = true;
        self.redraw();
    }

    /// How many lines a tee has written, as the session counts them.
    pub(super) fn tee_lines(&mut self, id: PaneId, lines: u64) {
        if let Some(p) = self.panes.get_mut(&id) {
            if let Some(t) = &mut p.tee {
                t.lines = lines as usize;
                t.armed = false;
                p.full = true;
                self.full = true;
                self.redraw();
            }
        }
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
    /// The minimize button: the pane shrunk to a row.
    Minimize,
    /// The restart button, on a pane running a command.
    Restart,
    /// The focus button: this pane alone, or all of them again.
    Focus,
    /// The close button: the pane closed, its terminal running on.
    Close,
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
