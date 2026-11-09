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

impl App {
    /// The project a pane's terminal is in: on this machine, from its
    /// folder; on another, none for now (its files are not here to look in).
    pub(super) fn project_of(&self, id: PaneId) -> Option<Project> {
        let p = self.panes.get(&id)?;
        if p.pty.machine != 0 {
            return None;
        }
        let root = repo_root(&p.pty.cwd()?)?;
        let name = root.file_name()?.to_string_lossy().into_owned();
        Some(project_named(&name))
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
