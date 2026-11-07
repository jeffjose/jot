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
            let text = automate::plain(&bytes);
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
        }
        let Some(p) = self.panes.get_mut(&id) else { return bytes };
        let Some(f) = &mut p.filter else { return bytes };
        let (shown, ended) = f.pass(&bytes);
        if ended {
            let (text, hidden) = (f.text.clone(), f.hidden);
            p.filter = None;
            p.full = true;
            self.toast("Filter ended", &format!("\"{text}\" · {hidden} line{} hidden", if hidden == 1 { "" } else { "s" }), None, Some(id));
        }
        shown
    }

    /// /tee FILE: the pane's output to the file as well, from now on.
    /// /tee alone: stopped.
    pub(super) fn tee(&mut self, id: PaneId, arg: &str) {
        let arg = arg.trim();
        if arg.is_empty() {
            return self.stop_tee(id);
        }
        let cwd = self.panes.get(&id).and_then(|p| p.pty.cwd()).unwrap_or_else(pty::home);
        let path = persist::expand(arg);
        let path = if path.is_absolute() { path } else { cwd.join(path) };
        match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => {
                if let Some(p) = self.panes.get_mut(&id) {
                    p.tee = Some(Tee { path: path.clone(), file, lines: 0, shown_at: Instant::now() });
                    p.full = true;
                }
            }
            Err(e) => self.toast("Could not tee", &format!("{}: {e}", persist::shorten(&path)), Some(0xE5484D), Some(id)),
        }
        self.full = true;
        self.redraw();
    }

    pub(super) fn stop_tee(&mut self, id: PaneId) {
        let Some(t) = self.panes.get_mut(&id).and_then(|p| p.tee.take()) else { return };
        if let Some(p) = self.panes.get_mut(&id) {
            p.full = true;
        }
        self.toast("Tee stopped", &format!("{} lines in {}", t.lines, persist::shorten(&t.path)), None, Some(id));
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
