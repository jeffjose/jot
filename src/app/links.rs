//! Links in what a pane shows: with Ctrl held, the URL under the pointer
//! is underlined, and a Ctrl click opens it in the browser, as in VS
//! Code's terminal. A URL that runs on past the end of a row, wrapped,
//! is one link across the rows. A path that is there on disk is a link
//! too: a file opens in your editor in a pane to the right, at the line
//! after it (`src/app.rs:42`), and a folder opens a pane in it.

use super::*;
use crate::vt::Span;
use std::path::PathBuf;

/// What a link goes to.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Url(String),
    /// A file, and the line named after it.
    File(PathBuf, Option<u32>),
    Folder(PathBuf),
}

pub struct Link {
    pub pane: PaneId,
    pub spans: Vec<Span>,
    pub target: Target,
}

/// The URLs in a line of text: where each starts and ends (end not
/// included), by character.
pub fn urls(line: &[char]) -> Vec<(usize, usize)> {
    let text: String = line.iter().collect();
    let mut out = Vec::new();
    let mut from = 0;
    while from < line.len() {
        let rest: String = line[from..].iter().collect();
        let Some(at) = ["https://", "http://", "file://", "ftp://"].iter().filter_map(|s| rest.find(s)).min() else { break };
        // Byte offset to character offset.
        let start = from + rest[..at].chars().count();
        let mut end = start;
        while end < line.len() && !line[end].is_whitespace() && !"<>\"'`|".contains(line[end]) {
            end += 1;
        }
        // A sentence's full stop or a closing bracket is not the link's,
        // unless the link opened that bracket.
        while end > start {
            let c = line[end - 1];
            let opened = |o: char, cl: char| c == cl && line[start..end].iter().filter(|&&x| x == o).count() >= line[start..end].iter().filter(|&&x| x == cl).count();
            if ".,;:!?".contains(c) || (")]}".contains(c) && !(opened('(', ')') || opened('[', ']') || opened('{', '}'))) {
                end -= 1;
            } else {
                break;
            }
        }
        if end - start > 8 {
            out.push((start, end));
        }
        from = end.max(start + 1);
    }
    let _ = text;
    out
}

impl App {
    /// The link under a cell of a pane, if there is one.
    pub(super) fn link_at(&self, id: PaneId, x: u16, y: u16) -> Option<Link> {
        let p = self.panes.get(&id)?;
        let lines = &p.lines;
        let row = |y: usize| lines.get(y).map(|r| r.as_slice()).unwrap_or(&[]);
        // A row that runs to its last column goes on in the next: the
        // rows of one line, wrapped.
        let full = |y: usize| row(y).last().is_some_and(|c| !c.is_whitespace());
        let mut top = y as usize;
        while top > 0 && full(top - 1) {
            top -= 1;
        }
        let mut bottom = y as usize;
        while bottom + 1 < lines.len() && full(bottom) {
            bottom += 1;
        }
        let cols = p.cols as usize;
        let mut joined: Vec<char> = Vec::new();
        for r in top..=bottom {
            let mut chars = row(r).to_vec();
            chars.resize(cols, ' ');
            joined.extend(chars);
        }
        let at = (y as usize - top) * cols + x as usize;
        let (start, end, target) = match urls(&joined).into_iter().find(|(s, e)| at >= *s && at < *e) {
            Some((s, e)) => (s, e, Target::Url(joined[s..e].iter().collect())),
            None => {
                let cwd = p.pty.cwd()?;
                path_at(&joined, at, &cwd)?
            }
        };
        let mut spans = Vec::new();
        let mut i = start;
        while i < end {
            let (ry, rx) = (i / cols, i % cols);
            let stop = ((ry + 1) * cols).min(end);
            spans.push(Span { y: (top + ry) as u16, x0: rx as u16, x1: (rx + stop - i - 1) as u16 });
            i = stop;
        }
        Some(Link { pane: id, spans, target })
    }

    /// With Ctrl held, the link under the pointer is the one lit.
    pub(super) fn update_link(&mut self) {
        let (x, y) = self.pointer;
        let found = if self.mods.control_key() && self.palette.is_none() && self.settings.is_none() && self.menu.is_none() {
            self.pane_at(x, y).and_then(|(id, (cx, cy))| self.link_at(id, cx, cy))
        } else {
            None
        };
        let before = self.link.as_ref().map(|l| (l.pane, l.spans.clone()));
        let after = found.as_ref().map(|l| (l.pane, l.spans.clone()));
        if before != after {
            for id in [before.map(|b| b.0), after.as_ref().map(|a| a.0)].into_iter().flatten() {
                if let Some(p) = self.panes.get_mut(&id) {
                    p.full = true;
                }
            }
            self.link = found;
            self.redraw();
        }
        if let (Some(w), Some(_)) = (&self.window, &self.link) {
            w.set_cursor(CursorIcon::Pointer);
        }
    }

    /// Opens the link lit, in the browser. True when there was one.
    pub(super) fn open_link(&mut self) -> bool {
        let Some(link) = self.link.take() else { return false };
        match link.target {
            Target::Url(url) => {
                let _ = std::process::Command::new("xdg-open").arg(&url).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
            }
            // A file opens in your editor in a pane to the right, at its line.
            Target::File(path, line) => {
                let editor = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).ok().filter(|e| !e.is_empty()).unwrap_or_else(|| "vi".into());
                let at = line.map(|n| format!(" +{n}")).unwrap_or_default();
                let cwd = path.parent().map(|p| p.to_path_buf()).unwrap_or_else(pty::home);
                let quoted = format!("'{}'", path.display().to_string().replace('\'', "'\\''"));
                self.split_with(Dir::Across, Start { cmd: format!("{editor}{at} {quoted}"), cwd, pane: None, host: None });
            }
            Target::Folder(path) => self.split_with(Dir::Across, Start { cmd: String::new(), cwd: path, pane: None, host: None }),
        }
        if let Some(p) = self.panes.get_mut(&link.pane) {
            p.full = true;
        }
        true
    }

    /// The link's underline in pane `id`, if it has the lit one.
    pub(super) fn link_spans(&self, id: PaneId) -> Vec<Span> {
        self.link.as_ref().filter(|l| l.pane == id).map(|l| l.spans.clone()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::urls;

    fn found(s: &str) -> Vec<String> {
        let c: Vec<char> = s.chars().collect();
        urls(&c).into_iter().map(|(a, b)| c[a..b].iter().collect()).collect()
    }

    #[test]
    fn finds_urls() {
        assert_eq!(found("see https://example.com/a?b=1."), vec!["https://example.com/a?b=1"]);
        assert_eq!(found("(https://en.wikipedia.org/wiki/Rust_(language))"), vec!["https://en.wikipedia.org/wiki/Rust_(language)"]);
        assert_eq!(found("a http://x.io/1 and https://y.io/2"), vec!["http://x.io/1", "https://y.io/2"]);
        assert!(found("no links here").is_empty());
    }
}

/// The path around character `at` of a line, if there is one on disk:
/// a word with a slash or a dot in it, `~` for home, relative to `cwd`,
/// with a `:line` (and `:column`) after it kept for a file.
pub fn path_at(line: &[char], at: usize, cwd: &std::path::Path) -> Option<(usize, usize, Target)> {
    let stop = |c: char| c.is_whitespace() || "\"'`<>()[]{}|,;".contains(c);
    if at >= line.len() || stop(line[at]) {
        return None;
    }
    let mut start = at;
    while start > 0 && !stop(line[start - 1]) {
        start -= 1;
    }
    let mut end = at;
    while end < line.len() && !stop(line[end]) {
        end += 1;
    }
    // A full stop or a colon at the end belongs to the sentence.
    while end > start && ".:".contains(line[end - 1]) {
        end -= 1;
    }
    let word: String = line[start..end].iter().collect();
    if !word.contains('/') && !word.contains('.') {
        return None;
    }
    // `path:line:col`, the numbers taken off.
    let mut parts: Vec<&str> = word.split(':').collect();
    let mut numbers = Vec::new();
    while parts.len() > 1 && parts.last().is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
        numbers.insert(0, parts.pop().unwrap().parse::<u32>().ok()?);
    }
    let name = parts.join(":");
    let name = name.strip_prefix("a/").or_else(|| name.strip_prefix("b/")).filter(|_| !std::path::Path::new(&name).exists()).unwrap_or(&name);
    let path = if let Some(rest) = name.strip_prefix("~/") {
        crate::pty::home().join(rest)
    } else {
        cwd.join(name)
    };
    let meta = std::fs::metadata(&path).ok()?;
    let target = if meta.is_dir() { Target::Folder(path) } else { Target::File(path, numbers.first().copied()) };
    Some((start, end, target))
}
