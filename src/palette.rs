//! The palette, as jog's: Ctrl K to find a workspace, a pane or a project
//! folder, Ctrl P for a command (what is typed after `>`). Enter to go.
//! `@` finds only workspaces. With nothing typed, what was run from it
//! lately comes first.

use std::path::PathBuf;

use crate::layout::Dir;

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Split(Dir),
    /// Split, the new pane before: left, or up.
    SplitBefore(Dir),
    Close,
    /// The terminal in the pane ended, the pane with it (Kill terminal);
    /// asked first when it runs a command, and then for sure.
    EndPane,
    KillTerminalNow,
    /// A terminal no pane here shows, by machine and its name there: shown
    /// in the pane you are in.
    ShowTerminal(usize, u64),
    /// The picker of those terminals; the pane given the next or the one
    /// before.
    PickTerminal,
    NewWindow,
    /// `jot restart --all`: the session, the quake window and every window
    /// as the jot installed now, each as it was.
    RestartAll,
    /// The quake window full width, centered (its width kept), its height
    /// the default again.
    QuakeFullWidth,
    QuakeCenter,
    QuakeResetHeight,
    QuakeHide,
    NewTerminalHere,
    NextTerminal,
    PrevTerminal,
    /// A terminal no pane here shows: in a split beside the pane you are
    /// in; or ended.
    SplitTerminal(usize, u64),
    /// A terminal shown in a new jot window too, as a link opened in one.
    OpenInNewWindow(usize, u64),
    KillFree(usize, u64),
    /// A terminal renamed, by machine and its name there.
    RenameTerminal(usize, u64),
    /// A machine in the sidebar: a new terminal on it; reached again;
    /// its terminals no pane shows ended; taken out of the sidebar.
    NewTerminalOn(usize),
    Reconnect(usize),
    KillFreeOn(usize),
    RemoveMachine(usize),
    /// The sidebar shown, or tucked away.
    Sidebar,
    /// A workspace on another machine: its session reached by ssh.
    Connect(String),
    /// A machine added to the sidebar.
    AddMachine(String),
    /// The box for a machine to connect to.
    ConnectPrompt,
    /// The window closed and every pane ended, the session with them.
    QuitAll,
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
    PopOut,
    CopyOutput,
    CopyCommand,
    /// The command under the menu: again, or into the editor first.
    Rerun,
    /// The command under the menu pinned to its pane; a pin run, edited
    /// or unpinned, by pane and place.
    Pin,
    PinRun(u64, usize),
    PinEdit(u64, usize),
    PinRemove(u64, usize),
    /// A minimized pane's row: back, back and alone, its command
    /// restarted or stopped, its last line copied, it closed.
    MiniRestore(u64),
    MiniFocus(u64),
    MiniRestart(u64),
    MiniStop(u64),
    MiniCopyLine(u64),
    MiniClose(u64),
    /// The pane under the menu made empty space; empty space a terminal
    /// again, or taken out.
    MakeSpace,
    SpaceToTerminal(u64),
    SpaceRemove(u64),
    EditCommand,
    /// The pane's running command, stopped and run again.
    Restart,
    CommandLine,
    /// The pane shrunk to a row at the top of its workspace.
    Minimize,
    /// From a caption's tags, for the pane with this id: typing into
    /// several, a tee, a filter.
    MultiSkip(u64),
    MultiAll,
    TeeStop(u64),
    TeeOpen(u64),
    TeeCopyPath(u64),
    FilterStop(u64),
    NotifyOff(u64),
    /// The text field of a pane's command line: its menu.
    FieldCut,
    FieldCopy,
    FieldPaste,
    FieldSelectAll,
    Settings,
    Palette,
    /// Opens the n-th submenu of the right-click menu.
    Submenu(usize),
    /// The workspace you are in given this accent, by name; None: chosen
    /// for it, as the settings' Color each workspace does.
    WsAccent(Option<String>),
    /// The project bar shows this machine's workspaces: the one you were
    /// in last there.
    ShowMachine(usize),
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
    /// A terminal's name, by machine and its name in that machine's session.
    RenameTerminal(usize, u64),
    /// A machine to make the pane a shell on: one of ~/.ssh/config's, or
    /// any typed.
    Machine,
    /// A machine to add to the sidebar.
    AddMachine,
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
            (Mode::Rename, _) | (Mode::RenameTerminal(..), _) => "Name:",
            (Mode::Machine, _) => "Machine:",
            (Mode::AddMachine, _) => "Add machine:",
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
        Some('>') => (typed[1..].trim_start(), Box::new(|i: &Item| !matches!(i.action, Action::GoTo(_) | Action::Open(_) | Action::Pane(..)))),
        Some('@') => (typed[1..].trim_start(), Box::new(|i: &Item| matches!(i.action, Action::GoTo(_)))),
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
        c("Split right", "Alt V", Action::Split(Dir::Across)),
        c("Split down", "Alt S", Action::Split(Dir::Down)),
        c("Split left", "Alt Shift V", Action::SplitBefore(Dir::Across)),
        c("Split up", "Alt Shift S", Action::SplitBefore(Dir::Down)),
        c("New pane", "Ctrl T", Action::NewPane),
        c("Close pane", "Ctrl W", Action::Close),
        c("Kill terminal (ends its shell)", "", Action::EndPane),
        c("Show another terminal here", "Alt '", Action::PickTerminal),
        c("New window", "Ctrl Shift N", Action::NewWindow),
        c("New terminal in this pane", "Alt N", Action::NewTerminalHere),
        c("Next terminal in this pane", "Alt .", Action::NextTerminal),
        c("Previous terminal in this pane", "Alt ,", Action::PrevTerminal),
        c("Pop out pane", "Alt Shift O", Action::PopOut),
        c("Reopen closed pane", "Ctrl Shift T", Action::Reopen),
        c("Maximize pane / restore", "Alt Shift F", Action::Zoom),
        c("Minimize pane", "", Action::Minimize),
        c("Equalize pane sizes", "Alt =", Action::Equalize),
        c("Close other panes", "", Action::Only),
        c("New workspace", "Ctrl N", Action::NewWorkspace),
        c("Rename workspace…", "", Action::Rename),
        c("Close workspace", "", Action::CloseWorkspace),
        c("Toggle roaming terminal", "Alt `", Action::Roam),
        c("Clear terminal", "", Action::Clear),
        c("Copy", "Ctrl Shift C", Action::Copy),
        c("Paste", "Ctrl Shift V", Action::Paste),
        c("Zoom in", "Ctrl =", Action::FontLarger),
        c("Zoom out", "Ctrl -", Action::FontSmaller),
        c("Reset zoom", "Ctrl 0", Action::FontReset),
        c("Toggle status line", "", Action::StatusLine),
        c("Type into multiple panes", "Alt Shift M", Action::Broadcast),
        c("Pane command line: run or queue", "Alt ;", Action::CommandLine),
        c("Settings", "Ctrl ,", Action::Settings),
        c("Connect to machine…", "", Action::ConnectPrompt),
        c("Show or hide the sidebar", "", Action::Sidebar),
        c("Restart jot (after an update): every window, terminals kept", "", Action::RestartAll),
        c("Quit and end all panes", "", Action::QuitAll),
        c("Go to workspace, pane or folder", "Ctrl K", Action::Palette),
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
        let found = matching(items.clone(), ">splr", &[]);
        assert_eq!(found[0].label, "Split right");
        let found = matching(items.clone(), ">", &["Paste".to_string()]);
        assert_eq!(found[0].label, "Paste");
        assert!(matching(items, ">zzzz", &[]).is_empty());
        // Plain words find places, not commands.
        assert!(matching(commands(), "split", &[]).is_empty());
    }
}
