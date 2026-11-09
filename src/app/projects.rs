//! Projects: what a terminal is working on, found by itself from the git
//! repository its folder is in (~/scripts/jot is jot, wherever its pane
//! is). A project has a mark, an icon and a color picked from its name, the
//! same every time, which its terminals wear in their captions and their
//! sidebar rows; a workspace that mixes projects keeps them apart.

use super::*;
use std::cell::RefCell;
use std::path::Path;

/// A terminal's project.
#[derive(Clone, PartialEq, Debug)]
pub struct Project {
    pub name: String,
    pub icon: char,
    /// Its color as the theme's accents have it.
    pub color: u32,
}

thread_local! {
    /// The repository each folder is in (None: none), looked up once.
    static ROOTS: RefCell<HashMap<PathBuf, Option<PathBuf>>> = RefCell::new(HashMap::new());
}

/// The top of the git repository `dir` is in: the nearest folder up from
/// it with a `.git` (a folder, or a worktree's file). Not the home folder
/// itself, which some keep their dotfiles in: everything would be in it.
fn repo_root(dir: &Path) -> Option<PathBuf> {
    if let Some(r) = ROOTS.with(|m| m.borrow().get(dir).cloned()) {
        return r;
    }
    let home = pty::home();
    let found = dir.ancestors().take_while(|d| *d != home && d.parent().is_some()).find(|d| d.join(".git").exists()).map(Path::to_path_buf);
    ROOTS.with(|m| m.borrow_mut().insert(dir.to_path_buf(), found.clone()));
    found
}

/// A number from a name, the same in every run (not Rust's hasher, which
/// is seeded anew each time).
fn stable_hash(s: &str) -> u64 {
    let h = s.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    // Mixed through (splitmix64's finish), so that names alike (jot, jog)
    // differ in every bit, not only the last few.
    let h = (h ^ (h >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    let h = (h ^ (h >> 27)).wrapping_mul(0x94d049bb133111eb);
    h ^ (h >> 31)
}

/// The project of that name, its icon and color picked from the name.
pub fn project_named(name: &str) -> Project {
    let h = stable_hash(name);
    let icons = crate::canvas::icons::PROJECT;
    // Of the theme's accents, eight far enough apart to tell at a glance
    // (red, ruby and crimson side by side would not be).
    let accents = [0x3E63DD, 0x00A2C7, 0x12A594, 0x46A758, 0xFFC53D, 0xF76B15, 0xD6409F, 0x8E4EC6];
    Project { name: name.to_string(), icon: icons[(h % icons.len() as u64) as usize], color: accents[((h >> 16) % accents.len() as u64) as usize] }
}

/// `name` without its project's name in front ("jot > displaybug", as
/// Claude Code titles a session, is "displaybug" beside jot's mark), and
/// whether there was anything else to it.
pub fn without_project<'a>(name: &'a str, project: &str) -> &'a str {
    let rest = name.strip_prefix(project).map(str::trim_start);
    match rest.and_then(|r| r.strip_prefix('>').or_else(|| r.strip_prefix('›')).or_else(|| r.strip_prefix(':'))) {
        Some(r) if !r.trim().is_empty() => r.trim(),
        _ => name,
    }
}

/// Projects set by hand: the terminals put in one (or in none) whatever
/// their folder, and the projects made that no folder is. Kept in the
/// state folder's `jot/projects`, a line each: `pane SID NAME` (`-` for
/// none), `project NAME`. This machine's terminals only, for now.
#[derive(Default, Debug, PartialEq)]
pub struct ByHand {
    pub panes: HashMap<u64, Option<String>>,
    pub made: Vec<String>,
}

impl ByHand {
    pub fn parse(text: &str) -> ByHand {
        let mut b = ByHand::default();
        for line in text.lines() {
            let mut w = line.splitn(3, ' ');
            match (w.next(), w.next(), w.next()) {
                (Some("pane"), Some(sid), Some(name)) => {
                    if let Ok(sid) = sid.parse() {
                        b.panes.insert(sid, (name != "-").then(|| name.to_string()));
                    }
                }
                (Some("project"), Some(a), rest) => {
                    let name = rest.map_or(a.to_string(), |r| format!("{a} {r}"));
                    if !b.made.contains(&name) {
                        b.made.push(name);
                    }
                }
                _ => {}
            }
        }
        b
    }

    pub fn text(&self) -> String {
        let mut sids: Vec<&u64> = self.panes.keys().collect();
        sids.sort();
        let mut out: String = self.made.iter().map(|m| format!("project {m}\n")).collect();
        for sid in sids {
            out += &format!("pane {sid} {}\n", self.panes[sid].as_deref().unwrap_or("-"));
        }
        out
    }
}

fn by_hand_path() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| pty::home().join(".local/state")).join("jot/projects")
}

thread_local! {
    static BY_HAND: RefCell<Option<ByHand>> = const { RefCell::new(None) };
}

/// What is set by hand, read once.
fn with_by_hand<R>(f: impl FnOnce(&mut ByHand) -> R) -> R {
    BY_HAND.with(|b| {
        let mut b = b.borrow_mut();
        let b = b.get_or_insert_with(|| ByHand::parse(&std::fs::read_to_string(by_hand_path()).unwrap_or_default()));
        f(b)
    })
}

fn save_by_hand() {
    let text = with_by_hand(|b| b.text());
    let path = by_hand_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, text);
}

impl App {
    /// The project a pane's terminal is in: the one it was put in by hand;
    /// else, on this machine, the repository its folder is in; on another,
    /// none for now (its files are not here to look in).
    pub(super) fn project_of(&self, id: PaneId) -> Option<Project> {
        let p = self.panes.get(&id)?;
        if p.pty.machine != 0 {
            return None;
        }
        let (_, sid) = crate::mux::split_id(id);
        if let Some(set) = with_by_hand(|b| b.panes.get(&sid).cloned()) {
            return set.map(|n| project_named(&n));
        }
        self.found_project(id)
    }

    /// A pane's project as its folder says, nothing set by hand counted.
    fn found_project(&self, id: PaneId) -> Option<Project> {
        let p = self.panes.get(&id)?;
        let root = repo_root(&p.pty.cwd()?)?;
        let name = root.file_name()?.to_string_lossy().into_owned();
        Some(project_named(&name))
    }

    /// Every project known here: those panes are in, made by hand, or set
    /// on a terminal; by name.
    pub(super) fn known_projects(&self) -> Vec<Project> {
        // A pane's project and its folder's both: one moved out of jot
        // leaves jot on the list.
        let mut names: Vec<String> = self.panes.keys().flat_map(|id| [self.project_of(*id), self.found_project(*id)]).flatten().map(|p| p.name).collect();
        with_by_hand(|b| {
            names.extend(b.made.iter().cloned());
            names.extend(b.panes.values().flatten().cloned());
        });
        names.sort_by_key(|n| n.to_lowercase());
        names.dedup();
        names.iter().map(|n| project_named(n)).collect()
    }

    /// The pane you are in put in a project by hand (None: in none). Put
    /// back in the one its folder is in, it is no longer set by hand.
    pub(super) fn set_project(&mut self, name: Option<String>) {
        let Some(id) = self.focused() else { return };
        if self.panes.get(&id).is_none_or(|p| p.pty.machine != 0) {
            return;
        }
        let (_, sid) = crate::mux::split_id(id);
        let found = self.found_project(id).map(|p| p.name);
        with_by_hand(|b| {
            if name == found {
                b.panes.remove(&sid);
            } else {
                b.panes.insert(sid, name.clone());
            }
            if let Some(n) = &name {
                if found.as_ref() != Some(n) && !b.made.contains(n) && !b.panes.values().flatten().any(|m| m == n) {
                    b.made.push(n.clone());
                }
            }
        });
        save_by_hand();
        self.full = true;
        self.side_dirty = true;
        self.redraw();
    }

    /// A project made, the pane you are in put in it.
    pub(super) fn new_project(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        with_by_hand(|b| {
            if !b.made.iter().any(|m| m == name) {
                b.made.push(name.to_string());
            }
        });
        self.set_project(Some(name.to_string()));
    }

    /// The projects, from a pane's badge: each a row (this pane put in
    /// it) with its mark and a cog (its settings); then none, and a new one.
    pub(super) fn open_project_menu(&mut self, id: PaneId, at: (i32, i32)) {
        let current = self.project_of(id).map(|p| p.name);
        let mut items = Vec::new();
        let mut icons = Vec::new();
        let mut cogs = Vec::new();
        for p in self.known_projects() {
            let here = current.as_ref() == Some(&p.name);
            items.push(Some(crate::palette::Item { label: p.name.clone(), hint: if here { crate::canvas::icons::CHECK.to_string() } else { String::new() }, action: crate::palette::Action::SetProject(Some(p.name.clone())) }));
            icons.push(Some((p.icon, p.color)));
            cogs.push(Some(crate::palette::Action::ProjectSettings(p.name.clone())));
        }
        if !items.is_empty() {
            items.push(None);
            icons.push(None);
            cogs.push(None);
        }
        items.push(Some(crate::palette::Item { label: "No project".into(), hint: if current.is_none() { crate::canvas::icons::CHECK.to_string() } else { String::new() }, action: crate::palette::Action::SetProject(None) }));
        items.push(Some(crate::palette::Item { label: "New project…".into(), hint: String::new(), action: crate::palette::Action::NewProject }));
        self.palette = None;
        self.menu_at = at;
        self.menu = Some(super::overlay::Menu { items, at, hover: None, children: Vec::new(), open: None, icons, cogs });
        self.full = true;
        self.redraw();
    }

    /// The box for a new project's name.
    pub(super) fn new_project_prompt(&mut self) {
        self.palette = Some(crate::palette::Palette { mode: crate::palette::Mode::NewProject, field: crate::field::TextField::default(), matches: Vec::new(), selected: 0, first: 0 });
        self.full = true;
        self.redraw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_lose_their_project() {
        assert_eq!(without_project("jot > displaybug", "jot"), "displaybug");
        assert_eq!(without_project("jot › perf", "jot"), "perf");
        assert_eq!(without_project("jot", "jot"), "jot");
        assert_eq!(without_project("jotter", "jot"), "jotter");
        assert_eq!(without_project("circadia", "jot"), "circadia");
    }

    // Reported: a pane could not be put in another project. Set by hand,
    // it stays there across runs (the file read back as written).
    #[test]
    fn projects_set_by_hand_are_kept() {
        let mut b = ByHand::default();
        b.panes.insert(7, Some("circadia".into()));
        b.panes.insert(9, None);
        b.made.push("side quest".into());
        let back = ByHand::parse(&b.text());
        assert_eq!(back, b);
        assert_eq!(back.panes[&7].as_deref(), Some("circadia"));
        assert_eq!(back.panes[&9], None);
    }

    #[test]
    fn a_name_always_gets_the_same_mark() {
        assert_eq!(project_named("jot"), project_named("jot"));
        assert_ne!(project_named("jot").icon, '\0');
    }
}

thread_local! {
    /// Each terminal's project as a pane here last showed it: its row keeps
    /// the mark while no pane does (its folder is not known here then).
    static SEEN: RefCell<HashMap<(usize, u64), Option<Project>>> = RefCell::new(HashMap::new());
}

/// A terminal's project, noted as a pane shows it.
pub fn remember(k: usize, sid: u64, project: Option<Project>) {
    SEEN.with(|m| m.borrow_mut().insert((k, sid), project));
}

/// A terminal's project as last noted, for its row while no pane shows it.
pub fn remembered(k: usize, sid: u64) -> Option<Project> {
    SEEN.with(|m| m.borrow().get(&(k, sid)).cloned().flatten())
}

impl App {
    /// A workspace's mark on the foot bar, in its color: the one you are
    /// in (`strong`) on a rounded square of the color, faint; the rest the
    /// mark by itself, nearly as big as `r`. A dot when it has no mark.
    pub(super) fn mark_tile(&mut self, r: Rect, color: u32, mark: Option<char>, strong: bool) {
        if strong {
            self.canvas.round_fill(r, (r.w as f32 * 0.28).round() as i32, color, 64);
        }
        // On a dark ground the color lifted a little, to read as well as it
        // does on a light one.
        let ink = if self.cur_theme.dark { mix(color, 0xFFFFFF, if strong { 77 } else { 64 }) } else { mix(color, 0x000000, 30) };
        match mark {
            Some(m) => {
                let size = (r.w as f32 * if strong { 0.72 } else { 0.93 }).round() as i32;
                self.canvas.icon_in(m, r, size, ink);
            }
            None => {
                let d = (r.w as f32 * 0.5).round() as f32;
                self.canvas.disc(r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0, d / 2.0, ink);
            }
        }
    }
}

/// Each workspace's own mark, from its name, as its color is (see
/// theme::workspace_accents): the same name the same mark on every run,
/// and no two of a machine's workspaces the same while there are marks
/// left. A workspace is not its project: several in one repository each
/// have their own.
pub fn workspace_marks(names: &[&str]) -> Vec<char> {
    let marks = crate::canvas::icons::WORKSPACE;
    let mut used: Vec<char> = Vec::new();
    names
        .iter()
        .map(|name| {
            let start = (stable_hash(name) % marks.len() as u64) as usize;
            let pick = (0..marks.len()).map(|k| marks[(start + k) % marks.len()]).find(|m| !used.contains(m)).unwrap_or(marks[start]);
            used.push(pick);
            pick
        })
        .collect()
}

#[cfg(test)]
mod workspace_marks_tests {
    use super::workspace_marks;

    // Reported: eight workspaces opened in one repository all wore its mark.
    #[test]
    fn workspaces_in_one_repository_each_have_their_own_mark() {
        let names = ["wild-orchid", "slow-ember", "bright-heron", "calm-summit", "rustic-moth", "gentle-beacon", "lucky-falcon", "misty-heron"];
        let marks = workspace_marks(&names);
        let mut seen = marks.clone();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), names.len(), "{marks:?}");
    }

    #[test]
    fn a_workspace_keeps_its_mark() {
        assert_eq!(workspace_marks(&["wild-orchid", "slow-ember"]), workspace_marks(&["wild-orchid", "slow-ember"]));
        assert_eq!(workspace_marks(&["wild-orchid"])[0], workspace_marks(&["wild-orchid", "slow-ember"])[0]);
    }
}
