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
