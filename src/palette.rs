//! The palette, as jog's: Ctrl K to find a workspace, a pane or a project
//! folder, Ctrl P for a command (what is typed after `>`). Enter to go.
//! `@` finds only workspaces. With nothing typed, what was run from it
//! lately comes first.

use std::path::PathBuf;

use crate::layout::Dir;

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Split(Dir),
    Close,
    NewPane,
    Reopen,
    Zoom,
    Equalize,
    Only,
    GoTo(usize),
    /// A pane, by its workspace and id.
    Pane(usize, u64),
    NewWorkspace,
    Open(PathBuf),
    Rename,
    CloseWorkspace,
    Roam,
    Clear,
    Copy,
    Paste,
    FontLarger,
    FontSmaller,
    FontReset,
    StatusLine,
    Broadcast,
    Settings,
    Palette,
    Commands,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub label: String,
    /// The key that does the same, or what kind of thing it is.
    pub hint: String,
    pub action: Action,
}

/// What the box is for: finding, or naming the workspace.
#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Find,
    Rename,
}

pub struct Palette {
    pub mode: Mode,
    pub field: crate::field::TextField,
    pub matches: Vec<Item>,
    pub selected: usize,
    /// The first row shown, when there are more than fit.
    pub first: usize,
}

impl Palette {
    pub fn new() -> Palette {
        Palette { mode: Mode::Find, field: crate::field::TextField::default(), matches: Vec::new(), selected: 0, first: 0 }
    }

    pub fn typed(&self) -> String {
        self.field.string()
    }

    pub fn label(&self) -> &'static str {
        match (self.mode, self.field.text.first().copied()) {
            (Mode::Rename, _) => "Name:",
            (_, None) if !self.matches.is_empty() => "Find:",
            (_, Some('>')) => "Run:",
            (_, Some('@')) => "Workspace:",
            _ => "Find:",
        }
    }

    /// Moves the pick by `by` rows, keeping it among the `rows` shown.
    pub fn step(&mut self, by: isize, rows: usize) {
        if self.matches.is_empty() {
            return;
        }
        let n = self.matches.len() as isize;
        self.selected = (self.selected as isize + by).clamp(0, n - 1) as usize;
        self.show_selected(rows);
    }

    pub fn show_selected(&mut self, rows: usize) {
        let rows = rows.max(1);
        if self.selected < self.first {
            self.first = self.selected;
        } else if self.selected >= self.first + rows {
            self.first = self.selected + 1 - rows;
        }
    }
}

/// How well `name` matches `query`, or None: whole words first, then the
/// start of a word, then anywhere; with `loose`, the letters in order
/// with others between.
pub fn score(name: &str, query: &str, loose: bool) -> Option<i32> {
    let (name, query) = (name.to_lowercase(), query.trim().to_lowercase());
    if query.is_empty() {
        return Some(0);
    }
    if let Some(at) = name.find(&query) {
        let word_start = at == 0 || !name[..at].ends_with(|c: char| c.is_alphanumeric());
        return Some(1000 - at as i32 + if word_start { 500 } else { 0 } - name.len() as i32);
    }
    // Every word typed found somewhere, in any order.
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.len() > 1 && words.iter().all(|w| name.contains(w)) {
        return Some(400 - name.len() as i32);
    }
    if !loose {
        return None;
    }
    let mut chars = name.chars();
    let mut gaps = 0;
    for q in query.chars().filter(|c| !c.is_whitespace()) {
        loop {
            match chars.next() {
                Some(c) if c == q => break,
                Some(_) => gaps += 1,
                None => return None,
            }
        }
    }
    Some(100 - gaps - name.len() as i32)
}

/// The items matching what is typed, the closest first; with nothing
/// typed, those run lately first and the rest in their order.
pub fn matching(all: Vec<Item>, typed: &str, recent: &[String]) -> Vec<Item> {
    let (query, filter): (&str, Box<dyn Fn(&Item) -> bool>) = match typed.chars().next() {
        Some('>') => (&typed[1..], Box::new(|i: &Item| !matches!(i.action, Action::GoTo(_) | Action::Open(_) | Action::Pane(..)))),
        Some('@') => (&typed[1..], Box::new(|i: &Item| matches!(i.action, Action::GoTo(_)))),
        // Plain words find places: workspaces, panes, project folders.
        _ => (typed, Box::new(|i: &Item| matches!(i.action, Action::GoTo(_) | Action::Open(_) | Action::Pane(..)))),
    };
    let all: Vec<Item> = all.into_iter().filter(|i| filter(i)).collect();
    let lately = |label: &str| recent.iter().position(|r| r == label).unwrap_or(usize::MAX);
    if query.trim().is_empty() {
        let mut all = all;
        all.sort_by_key(|i| lately(&i.label));
        return all;
    }
    for loose in [false, true] {
        let mut scored: Vec<(i32, Item)> = all.iter().filter_map(|i| Some((score(&i.label, query, loose)?, i.clone()))).collect();
        if !scored.is_empty() {
            scored.sort_by_key(|(s, i)| (-s, lately(&i.label)));
            return scored.into_iter().map(|(_, i)| i).collect();
        }
    }
    Vec::new()
}

/// The commands, each with its key.
pub fn commands() -> Vec<Item> {
    let c = |label: &str, hint: &str, action| Item { label: label.into(), hint: hint.into(), action };
    vec![
        c("Split side by side", "Alt V", Action::Split(Dir::Across)),
        c("Split one above another", "Alt S", Action::Split(Dir::Down)),
        c("New pane", "Ctrl T", Action::NewPane),
        c("Close pane", "Ctrl W", Action::Close),
        c("Reopen closed pane", "Ctrl Shift T", Action::Reopen),
        c("Pane alone, and back", "Alt Shift M", Action::Zoom),
        c("Make panes equal", "Alt =", Action::Equalize),
        c("Close every other pane", "", Action::Only),
        c("New workspace here", "Alt Shift N", Action::NewWorkspace),
        c("Rename workspace", "", Action::Rename),
        c("Close workspace", "", Action::CloseWorkspace),
        c("Roaming terminal", "Alt `", Action::Roam),
        c("Clear screen and scrollback", "", Action::Clear),
        c("Copy", "Ctrl Shift C", Action::Copy),
        c("Paste", "Ctrl Shift V", Action::Paste),
        c("Text larger", "Ctrl =", Action::FontLarger),
        c("Text smaller", "Ctrl -", Action::FontSmaller),
        c("Text as it was", "Ctrl 0", Action::FontReset),
        c("Status line, on or off", "", Action::StatusLine),
        c("Type into every pane of the workspace, or one", "Alt Shift B", Action::Broadcast),
        c("Settings", "Ctrl ,", Action::Settings),
        c("Find a workspace, pane or project", "Ctrl K", Action::Palette),
    ]
}

/// The folders in each of `roots`: the projects that can be opened.
pub fn projects(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else { continue };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .map(|e| e.path())
            .collect();
        dirs.sort();
        out.extend(dirs);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closest_first() {
        let items = commands();
        let found = matching(items.clone(), ">split", &[]);
        assert!(found[0].label.starts_with("Split"));
        let found = matching(items.clone(), ">spsd", &[]);
        assert_eq!(found[0].label, "Split side by side");
        let found = matching(items.clone(), ">", &["Paste".to_string()]);
        assert_eq!(found[0].label, "Paste");
        assert!(matching(items, ">zzzz", &[]).is_empty());
        // Plain words find places, not commands.
        assert!(matching(commands(), "split", &[]).is_empty());
    }
}
