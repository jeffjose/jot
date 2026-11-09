//! Projects: what a terminal is working on, found by itself from the git
//! repository its folder is in (~/scripts/jot is jot, wherever its pane
//! is). A project has a mark, an icon and a color picked from its name, the
//! same every time, which its terminals wear in their captions and their
//! sidebar rows; a workspace that mixes projects keeps them apart.

use super::*;
use std::cell::RefCell;
use std::collections::BTreeMap;
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

/// The marks' names, in the order of canvas::icons::PROJECT.
pub const MARK_NAMES: [&str; 16] = ["Pen", "Moon", "Server", "Code", "Play", "Gallery", "Leaf", "Bolt", "Book", "Globe", "Box", "Star", "Heart", "Cup", "Rocket", "Palette"];

/// The colors a project or a pane can be given: the eight it may get from
/// its name.
pub const COLORS: [(u32, &str); 8] = [(0x3E63DD, "Indigo"), (0x00A2C7, "Cyan"), (0x12A594, "Teal"), (0x46A758, "Grass"), (0xFFC53D, "Amber"), (0xF76B15, "Orange"), (0xD6409F, "Pink"), (0x8E4EC6, "Purple")];

/// A project as Settings › Projects shows it.
pub struct ProjectRow {
    pub project: Project,
    /// The repositories its terminals are in, shortened (~/scripts/jot).
    pub folders: Vec<String>,
    /// The terminals put in it by hand: their id there, and name.
    pub by_hand: Vec<(u64, String)>,
    /// What it was given: its look, and the folders added to it.
    pub look: Look,
    pub configured: Vec<String>,
}

/// A look given by hand, to a project or to one pane: each part unset
/// keeps what it would be (a project's from its name, a pane's its
/// project's). A pane's tint is not used.
#[derive(Default, Debug, PartialEq, Clone)]
pub struct Look {
    pub icon: Option<char>,
    pub color: Option<u32>,
    /// A theme's name.
    pub theme: Option<String>,
    /// Its panes' background washed toward its color, 0 to 100.
    pub tint: Option<u8>,
}

impl Look {
    pub fn is_empty(&self) -> bool {
        *self == Look::default()
    }

    fn set(&mut self, key: &str, value: &str) {
        match key {
            "icon" => self.icon = u32::from_str_radix(value, 16).ok().and_then(char::from_u32),
            "color" => self.color = u32::from_str_radix(value, 16).ok(),
            "theme" => self.theme = Some(value.to_string()),
            "tint" => self.tint = value.parse().ok(),
            _ => {}
        }
    }

    fn lines(&self, kind: &str, who: &str) -> String {
        let mut out = String::new();
        if let Some(i) = self.icon {
            out += &format!("{kind}\t{who}\ticon\t{:x}\n", i as u32);
        }
        if let Some(c) = self.color {
            out += &format!("{kind}\t{who}\tcolor\t{c:06X}\n");
        }
        if let Some(t) = &self.theme {
            out += &format!("{kind}\t{who}\ttheme\t{t}\n");
        }
        if let Some(t) = self.tint {
            out += &format!("{kind}\t{who}\ttint\t{t}\n");
        }
        out
    }
}

/// Projects set by hand: the terminals put in one (or in none) whatever
/// their folder, the projects made that no folder is, each project's look
/// and folders, and each pane's own look. Kept in the state folder's
/// `jot/projects`, a line each: `pane SID NAME` (`-` for none), `project
/// NAME`; then, split by tabs, `look NAME KEY VALUE`, `folder NAME PATH`,
/// `panelook SID KEY VALUE`. This machine's terminals only, for now.
#[derive(Default, Debug, PartialEq)]
pub struct ByHand {
    pub panes: HashMap<u64, Option<String>>,
    pub made: Vec<String>,
    pub looks: BTreeMap<String, Look>,
    /// Each project's folders, as written (~/scripts/jot).
    pub folders: BTreeMap<String, Vec<String>>,
    pub pane_looks: HashMap<u64, Look>,
    /// The sidebar by project, not as a list.
    pub by_project: bool,
}

impl ByHand {
    pub fn parse(text: &str) -> ByHand {
        let mut b = ByHand::default();
        for line in text.lines() {
            if line.contains('\t') {
                let w: Vec<&str> = line.split('\t').collect();
                match w.as_slice() {
                    ["look", name, key, value] => b.looks.entry(name.to_string()).or_default().set(key, value),
                    ["view", "projects"] => b.by_project = true,
                    ["folder", name, path] => {
                        let f = b.folders.entry(name.to_string()).or_default();
                        if !f.iter().any(|p| p == path) {
                            f.push(path.to_string());
                        }
                    }
                    ["panelook", sid, key, value] => {
                        if let Ok(sid) = sid.parse() {
                            b.pane_looks.entry(sid).or_default().set(key, value);
                        }
                    }
                    _ => {}
                }
                continue;
            }
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
        for (name, look) in &self.looks {
            out += &look.lines("look", name);
        }
        for (name, folders) in &self.folders {
            for f in folders {
                out += &format!("folder\t{name}\t{f}\n");
            }
        }
        if self.by_project {
            out += "view\tprojects\n";
        }
        let mut sids: Vec<&u64> = self.pane_looks.keys().collect();
        sids.sort();
        for sid in sids {
            out += &self.pane_looks[sid].lines("panelook", &sid.to_string());
        }
        out
    }
}

/// A project's look: its mark and color from its name, unless given.
pub fn project_look(name: &str) -> Project {
    let mut p = project_named(name);
    with_by_hand(|b| {
        if let Some(l) = b.looks.get(name) {
            p.icon = l.icon.unwrap_or(p.icon);
            p.color = l.color.unwrap_or(p.color);
        }
    });
    p
}

/// What is set for a project (its look, folders), and for a pane.
pub fn look_of(name: &str) -> Look {
    with_by_hand(|b| b.looks.get(name).cloned().unwrap_or_default())
}

pub fn folders_of(name: &str) -> Vec<String> {
    with_by_hand(|b| b.folders.get(name).cloned().unwrap_or_default())
}

pub fn pane_look(sid: u64) -> Look {
    with_by_hand(|b| b.pane_looks.get(&sid).cloned().unwrap_or_default())
}

/// A change to what is set by hand, written at once.
pub fn change(f: impl FnOnce(&mut ByHand)) {
    with_by_hand(f);
    save_by_hand();
}

/// The project whose folders hold `dir` (the deepest, when several do).
fn project_by_folder(dir: &Path) -> Option<String> {
    with_by_hand(|b| {
        b.folders
            .iter()
            .flat_map(|(name, fs)| fs.iter().map(move |f| (name, persist::expand(f))))
            .filter(|(_, f)| dir.starts_with(f))
            .max_by_key(|(_, f)| f.components().count())
            .map(|(n, _)| n.clone())
    })
}

/// A theme washed toward a color (a project's tint), `share` of 255; one
/// made for each, kept for the run.
pub fn tinted(base: &'static crate::theme::Theme, color: u32, share: u32) -> &'static crate::theme::Theme {
    thread_local! {
        static MADE: RefCell<HashMap<(usize, u32, u32), &'static crate::theme::Theme>> = RefCell::new(HashMap::new());
    }
    MADE.with(|m| {
        *m.borrow_mut().entry((base as *const _ as usize, color, share)).or_insert_with(|| {
            let mut t = *base;
            t.bg = crate::canvas::mix(base.bg, color, share);
            Box::leak(Box::new(t))
        })
    })
}

fn by_hand_path() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| pty::home().join(".local/state")).join("jot/projects")
}

thread_local! {
    static BY_HAND: RefCell<Option<ByHand>> = const { RefCell::new(None) };
}

/// This machine's session, as when its server started (its socket made):
/// a terminal's id is its own only within one, the next numbering them
/// from 1 again. 0 when it cannot be told.
fn session_stamp() -> u64 {
    std::fs::metadata(crate::mux::socket()).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

/// The file's text read for this session: what was set on terminals of
/// another session (its ids now another terminal's, or none's) let go.
fn read_for_session(text: &str, now: u64) -> ByHand {
    let mut b = ByHand::parse(text);
    let then = text.lines().find_map(|l| l.strip_prefix("session\t")).and_then(|s| s.parse::<u64>().ok());
    if now != 0 && then.is_some_and(|t| t != now) {
        b.panes.clear();
        b.pane_looks.clear();
    }
    b
}

/// What is set by hand, read once.
fn with_by_hand<R>(f: impl FnOnce(&mut ByHand) -> R) -> R {
    BY_HAND.with(|b| {
        let mut b = b.borrow_mut();
        let b = b.get_or_insert_with(|| read_for_session(&std::fs::read_to_string(by_hand_path()).unwrap_or_default(), session_stamp()));
        f(b)
    })
}

fn save_by_hand() {
    let now = session_stamp();
    let text = format!("{}{}", if now != 0 { format!("session\t{now}\n") } else { String::new() }, with_by_hand(|b| b.text()));
    let path = by_hand_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, text);
}

impl App {
    /// The project a pane's terminal is in: the one it was put in by hand
    /// (on this machine); else the one its folder is in (found_project).
    pub(super) fn project_of(&self, id: PaneId) -> Option<Project> {
        let p = self.panes.get(&id)?;
        if p.pty.machine == 0 {
            let (_, sid) = crate::mux::split_id(id);
            if let Some(set) = with_by_hand(|b| b.panes.get(&sid).cloned()) {
                return set.map(|n| project_look(&n));
            }
        }
        self.found_project(id)
    }

    /// A pane's project as its folder says, nothing set by hand counted:
    /// a project whose folders hold it (Settings › Projects); on this
    /// machine, the git repository it is in; on another, whose files are
    /// not here to look in, a folder of a project's name on its way down
    /// (~/src/jot there is jot, as ~/scripts/jot is here).
    pub(super) fn found_project(&self, id: PaneId) -> Option<Project> {
        let p = self.panes.get(&id)?;
        let cwd = p.pty.cwd()?;
        if let Some(name) = project_by_folder(&cwd) {
            return Some(project_look(&name));
        }
        if p.pty.machine == 0 {
            let root = repo_root(&cwd)?;
            let name = root.file_name()?.to_string_lossy().into_owned();
            return Some(project_look(&name));
        }
        let names = self.local_project_names();
        named_on_path(&cwd, &names).map(|n| project_look(&n))
    }

    /// A pane's theme: its own, else its project's, else `ws` (its
    /// workspace's); its project's tint over that.
    pub(super) fn pane_theme(&self, id: PaneId, ws: &'static crate::theme::Theme) -> &'static crate::theme::Theme {
        if let Some((_, t)) = self.theme_preview.filter(|(p, _)| *p == id && self.menu.is_some()) {
            return t;
        }
        let project = self.project_of(id);
        let plook = project.as_ref().map(|p| look_of(&p.name)).unwrap_or_default();
        let own = self.panes.get(&id).filter(|p| p.pty.machine == 0).map(|_| pane_look(crate::mux::split_id(id).1)).unwrap_or_default();
        let base = own.theme.or(plook.theme).map(|n| crate::theme::Theme::by_name(&n)).unwrap_or(ws);
        match (project, plook.tint) {
            (Some(p), Some(t)) if t > 0 => tinted(base, p.color, (t as u32 * 64 / 100).min(64)),
            _ => base,
        }
    }

    /// A pane's badge: its project's mark and color, or its own where it
    /// has one; and whether it differs from its project in anything.
    pub(super) fn badge_of(&self, id: PaneId) -> Option<(Project, bool)> {
        let mut p = self.project_of(id)?;
        let own = self.panes.get(&id).filter(|p| p.pty.machine == 0).map(|_| pane_look(crate::mux::split_id(id).1)).unwrap_or_default();
        p.icon = own.icon.unwrap_or(p.icon);
        p.color = own.color.unwrap_or(p.color);
        Some((p, !own.is_empty()))
    }

    /// The projects this machine knows of by itself: its panes' repositories,
    /// and those made, set or given a look by hand.
    fn local_project_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .panes
            .values()
            .filter(|p| p.pty.machine == 0)
            .filter_map(|p| repo_root(&p.pty.cwd()?))
            .filter_map(|r| Some(r.file_name()?.to_string_lossy().into_owned()))
            .collect();
        with_by_hand(|b| {
            names.extend(b.made.iter().cloned());
            names.extend(b.panes.values().flatten().cloned());
            names.extend(b.looks.keys().cloned());
            names.extend(b.folders.keys().cloned());
        });
        names.sort();
        names.dedup();
        names
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
            names.extend(b.folders.keys().cloned());
        });
        names.sort_by_key(|n| n.to_lowercase());
        names.dedup();
        names.iter().map(|n| project_look(n)).collect()
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

    /// Each project known here as Settings › Projects lists it: the
    /// folders its terminals are in (their repositories), and the
    /// terminals put in it by hand, by name.
    pub(super) fn project_rows(&self) -> Vec<ProjectRow> {
        let by_hand: Vec<(u64, Option<String>)> = with_by_hand(|b| b.panes.iter().map(|(s, n)| (*s, n.clone())).collect());
        self.known_projects()
            .into_iter()
            .map(|project| {
                let mut folders: Vec<String> = self
                    .panes
                    .iter()
                    .filter(|(id, p)| p.pty.machine == 0 && self.found_project(**id).is_some_and(|f| f.name == project.name))
                    .filter_map(|(_, p)| repo_root(&p.pty.cwd()?))
                    .map(|r| persist::shorten(&r))
                    .collect();
                folders.sort();
                folders.dedup();
                let mut hand: Vec<(u64, String)> = by_hand
                    .iter()
                    .filter(|(_, n)| n.as_ref() == Some(&project.name))
                    .map(|(sid, _)| {
                        let shown = self.panes.keys().find(|id| crate::mux::split_id(**id) == (0, *sid)).map(|id| self.terminal_label(*id));
                        (*sid, shown.unwrap_or_else(|| format!("terminal {sid}")))
                    })
                    .collect();
                hand.sort();
                let (look, configured) = (look_of(&project.name), folders_of(&project.name));
                folders.retain(|f| !configured.contains(f));
                ProjectRow { project, folders, by_hand: hand, look, configured }
            })
            .collect()
    }

    /// A terminal set to a project by hand set back: in its folder's again.
    pub(super) fn set_back(&mut self, sid: u64) {
        with_by_hand(|b| b.panes.remove(&sid));
        save_by_hand();
        self.full = true;
        self.side_dirty = true;
        self.redraw();
    }

    /// A pane's cog (beside its name): this pane only. Its name, its icon
    /// and color, its theme, which project it is in, and all of it its
    /// project's again.
    pub(super) fn open_pane_menu(&mut self, id: PaneId, at: (i32, i32)) {
        use crate::palette::{Action, Item};
        let c = |label: &str, hint: String, action| Some(Item { label: label.into(), hint, action });
        let (k, sid) = crate::mux::split_id(id);
        let here = k == 0;
        let own = if here { pane_look(sid) } else { Look::default() };
        let by_hand = here && with_by_hand(|b| b.panes.contains_key(&sid));
        let project = self.project_of(id).map(|p| p.name);
        let themes = self.pane_theme_items(id);
        let mut items = vec![c("Rename…", String::new(), Action::RenameTerminal(k, sid))];
        if here {
            items.extend([
                None,
                c("Icon…", String::new(), Action::PaneIconMenu),
                c("Color…", String::new(), Action::PaneColorMenu),
                c("Theme", "›".into(), Action::Submenu(0)),
                None,
                c("Move to project…", project.clone().unwrap_or_default(), Action::MoveToProject),
            ]);
            if !own.is_empty() || by_hand {
                items.push(c("Reset to project", String::new(), Action::ResetPane));
            }
        }
        self.palette = None;
        self.menu_at = at;
        self.menu = Some(super::overlay::Menu { items, at, hover: None, children: vec![themes], open: None, icons: Vec::new(), cogs: Vec::new() });
        self.full = true;
        self.redraw();
    }

    /// A pane's themes, for a Theme submenu: its project's (or its
    /// workspace's) first, then each; the one it has ticked. Hovered, one
    /// is shown on the pane (see theme_preview).
    pub(super) fn pane_theme_items(&self, id: PaneId) -> Vec<Option<crate::palette::Item>> {
        use crate::palette::{Action, Item};
        let (k, sid) = crate::mux::split_id(id);
        let own = if k == 0 { pane_look(sid).theme } else { None };
        let tick = crate::canvas::icons::CHECK.to_string();
        let c = |label: &str, on: bool, action| Some(Item { label: label.into(), hint: if on { tick.clone() } else { String::new() }, action });
        let mut themes = vec![c(if self.project_of(id).is_some() { "As its project" } else { "As the workspace" }, own.is_none(), Action::PaneTheme(None)), None];
        themes.extend(crate::theme::THEMES.iter().map(|t| c(t.name, own.as_deref() == Some(t.name), Action::PaneTheme(Some(t.name.to_string())))));
        themes
    }

    /// The marks, for the pane you are in: its project's first.
    pub(super) fn open_pane_icon_menu(&mut self) {
        use crate::palette::{Action, Item};
        let Some(id) = self.focused() else { return };
        let Some((badge, _)) = self.badge_of(id) else { return };
        let own = pane_look(crate::mux::split_id(id).1).icon;
        let tick = crate::canvas::icons::CHECK.to_string();
        let base = self.project_of(id);
        let mut items = vec![Some(Item { label: "As its project".into(), hint: if own.is_none() { tick.clone() } else { String::new() }, action: Action::PaneIcon(None) }), None];
        let mut icons = vec![base.as_ref().map(|p| (p.icon, badge.color)), None];
        for (glyph, name) in crate::canvas::icons::PROJECT.iter().zip(MARK_NAMES) {
            items.push(Some(Item { label: name.into(), hint: if own == Some(*glyph) { tick.clone() } else { String::new() }, action: Action::PaneIcon(Some(*glyph)) }));
            icons.push(Some((*glyph, badge.color)));
        }
        let at = self.menu_at;
        self.menu = Some(super::overlay::Menu { items, at, hover: None, children: Vec::new(), open: None, icons, cogs: Vec::new() });
        self.full = true;
        self.redraw();
    }

    /// The colors, for the pane you are in: its project's first.
    pub(super) fn open_pane_color_menu(&mut self) {
        use crate::palette::{Action, Item};
        let Some(id) = self.focused() else { return };
        let Some((badge, _)) = self.badge_of(id) else { return };
        let own = pane_look(crate::mux::split_id(id).1).color;
        let tick = crate::canvas::icons::CHECK.to_string();
        let base = self.project_of(id);
        let mut items = vec![Some(Item { label: "As its project".into(), hint: if own.is_none() { tick.clone() } else { String::new() }, action: Action::PaneColor(None) }), None];
        let mut icons = vec![base.as_ref().map(|p| (badge.icon, p.color)), None];
        for (color, name) in COLORS {
            items.push(Some(Item { label: name.into(), hint: if own == Some(color) { tick.clone() } else { String::new() }, action: Action::PaneColor(Some(color)) }));
            icons.push(Some((badge.icon, color)));
        }
        let at = self.menu_at;
        self.menu = Some(super::overlay::Menu { items, at, hover: None, children: Vec::new(), open: None, icons, cogs: Vec::new() });
        self.full = true;
        self.redraw();
    }

    /// A part of the pane you are in's own look set (or let go of).
    pub(super) fn set_pane_look(&mut self, f: impl FnOnce(&mut Look)) {
        let Some(id) = self.focused() else { return };
        let (k, sid) = crate::mux::split_id(id);
        if k != 0 {
            return;
        }
        change(|b| {
            let l = b.pane_looks.entry(sid).or_default();
            f(l);
            if l.is_empty() {
                b.pane_looks.remove(&sid);
            }
        });
        if let Some(p) = self.panes.get_mut(&id) {
            p.full = true;
        }
        self.full = true;
        self.side_dirty = true;
        self.redraw();
    }

    /// The pane you are in as its project has it again: no look of its
    /// own, in the project its folder is in.
    pub(super) fn reset_pane(&mut self) {
        let Some(id) = self.focused() else { return };
        let (k, sid) = crate::mux::split_id(id);
        if k != 0 {
            return;
        }
        change(|b| {
            b.pane_looks.remove(&sid);
            b.panes.remove(&sid);
        });
        self.full = true;
        self.side_dirty = true;
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

    // A pane's look and project kept with its look and folders; a new
    // session (its terminals numbered from 1 again) lets the panes' go,
    // the projects' kept.
    #[test]
    fn looks_and_folders_are_kept_and_panes_only_within_their_session() {
        let mut b = ByHand::default();
        b.panes.insert(1, Some("jot".into()));
        b.pane_looks.insert(1, Look { color: Some(0x12A594), theme: Some("Rosé Pine".into()), ..Look::default() });
        b.looks.insert("side quest".into(), Look { icon: Some('\u{e31b}'), tint: Some(30), ..Look::default() });
        b.folders.insert("jot".into(), vec!["~/scripts/jot".into(), "~/scripts/jot-mach".into()]);
        let text = format!("session\t100\n{}", b.text());
        assert_eq!(read_for_session(&text, 100), b);
        let next = read_for_session(&text, 200);
        assert!(next.panes.is_empty() && next.pane_looks.is_empty());
        assert_eq!(next.looks, b.looks);
        assert_eq!(next.folders, b.folders);
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

/// Whether the sidebar shows its terminals by project.
pub fn sidebar_by_project() -> bool {
    with_by_hand(|b| b.by_project)
}

/// On another machine, the project a folder is in: the nearest folder on
/// its way down named as a project known here (~/src/jot/tools is jot).
pub fn named_on_path(dir: &Path, names: &[String]) -> Option<String> {
    dir.ancestors().filter_map(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()).find(|n| names.contains(n))
}

#[cfg(test)]
mod remote_tests {
    use super::named_on_path;
    use std::path::Path;

    // A terminal on another machine, in a clone of a project known here.
    #[test]
    fn a_remote_folder_is_in_the_project_it_is_named_for() {
        let names = vec!["jot".to_string(), "circadia".to_string()];
        assert_eq!(named_on_path(Path::new("/home/me/src/jot/tools"), &names).as_deref(), Some("jot"));
        assert_eq!(named_on_path(Path::new("/srv/circadia"), &names).as_deref(), Some("circadia"));
        assert_eq!(named_on_path(Path::new("/tmp/elsewhere"), &names), None);
    }
}
