//! What is kept between runs. Settings and workspaces you write yourself
//! live in ~/.config/jterm/config.toml; the workspaces as you left them
//! are saved as you go to ~/.local/state/jterm/session.toml, and brought
//! back when jterm starts: each pane's command, started afresh in the
//! folder it was in.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::layout::{Dir, Node, PaneId};
use crate::pty::home;

/// A layout as written in a file. A pane is its command ("" for your
/// shell), or a table with the folder it was in; a split is a table of
/// `v` (side by side) or `h` (stacked) panes, with their shares.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Spec {
    Command(String),
    Across {
        v: Vec<Spec>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        sizes: Vec<f32>,
    },
    Down {
        h: Vec<Spec>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        sizes: Vec<f32>,
    },
    Pane {
        #[serde(default)]
        cmd: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceSpec {
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The shorthand: panes in a row, `split` deciding which way.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub panes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<Dir>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<Spec>,
}

/// A pane to be started: its command and folder.
pub struct Start {
    pub cmd: String,
    pub cwd: PathBuf,
}

impl WorkspaceSpec {
    pub fn folder(&self) -> PathBuf {
        self.cwd.as_deref().map(expand).unwrap_or_else(home)
    }

    /// The layout, with `start` called for each pane to be made, giving
    /// back its id.
    pub fn build(&self, start: &mut impl FnMut(Start) -> PaneId) -> Node {
        let cwd = self.folder();
        let spec = match (&self.layout, self.panes.is_empty()) {
            (Some(l), _) => l.clone(),
            (None, false) if self.panes.len() == 1 => Spec::Command(self.panes[0].clone()),
            (None, false) => match self.split.unwrap_or(Dir::Across) {
                Dir::Across => Spec::Across { v: self.panes.iter().cloned().map(Spec::Command).collect(), sizes: vec![] },
                Dir::Down => Spec::Down { h: self.panes.iter().cloned().map(Spec::Command).collect(), sizes: vec![] },
            },
            (None, true) => Spec::Command(String::new()),
        };
        build(&spec, &cwd, start)
    }
}

fn build(spec: &Spec, cwd: &Path, start: &mut impl FnMut(Start) -> PaneId) -> Node {
    let split = |dir, kids: &Vec<Spec>, sizes: &Vec<f32>, start: &mut dyn FnMut(&Spec) -> Node| {
        if kids.is_empty() {
            return None;
        }
        let children: Vec<Node> = kids.iter().map(|k| start(k)).collect();
        let n = children.len();
        let sizes = if sizes.len() == n && sizes.iter().all(|s| *s > 0.0) {
            let total: f32 = sizes.iter().sum();
            sizes.iter().map(|s| s / total).collect()
        } else {
            vec![1.0 / n as f32; n]
        };
        Some(if n == 1 { children.into_iter().next().unwrap() } else { Node::Split { dir, children, sizes } })
    };
    match spec {
        Spec::Command(cmd) => Node::Pane(start(Start { cmd: cmd.clone(), cwd: cwd.to_path_buf() })),
        Spec::Pane { cmd, cwd: dir } => {
            let cwd = dir.as_deref().map(expand).unwrap_or_else(|| cwd.to_path_buf());
            Node::Pane(start(Start { cmd: cmd.clone(), cwd }))
        }
        Spec::Across { v, sizes } => split(Dir::Across, v, sizes, &mut |k| build(k, cwd, start)).unwrap_or_else(|| build(&Spec::Command(String::new()), cwd, start)),
        Spec::Down { h, sizes } => split(Dir::Down, h, sizes, &mut |k| build(k, cwd, start)).unwrap_or_else(|| build(&Spec::Command(String::new()), cwd, start)),
    }
}

/// A layout as it is now, to be saved: `pane` gives each pane's command
/// and the folder it is in.
pub fn spec(node: &Node, pane: &impl Fn(PaneId) -> (String, Option<PathBuf>)) -> Spec {
    match node {
        Node::Pane(id) => {
            let (cmd, cwd) = pane(*id);
            Spec::Pane { cmd, cwd: cwd.map(|p| shorten(&p)) }
        }
        Node::Split { dir, children, sizes } => {
            let kids = children.iter().map(|c| spec(c, pane)).collect();
            let sizes = sizes.iter().map(|s| (s * 1000.0).round() / 1000.0).collect();
            match dir {
                Dir::Across => Spec::Across { v: kids, sizes },
                Dir::Down => Spec::Down { h: kids, sizes },
            }
        }
    }
}

/// `~` at the front of a path, as home.
pub fn expand(path: &str) -> PathBuf {
    match path.strip_prefix('~') {
        Some(rest) => home().join(rest.trim_start_matches('/')),
        None => PathBuf::from(path),
    }
}

/// Home at the front of a path, as `~`.
pub fn shorten(path: &Path) -> String {
    match path.strip_prefix(home()) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

fn default_font_size() -> f32 {
    14.0
}
fn default_scrollback() -> usize {
    40960
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    #[serde(default = "default_scrollback")]
    pub scrollback: usize,
    /// A status line along the bottom: host, workspaces, clock.
    #[serde(default)]
    pub status_line: bool,
    /// A caption along the bottom of each pane.
    #[serde(default = "yes")]
    pub captions: bool,
    /// The colour theme, by name.
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Folders whose folders are projects, offered by the palette to open
    /// as workspaces.
    #[serde(default = "default_projects")]
    pub projects: Vec<String>,
    /// Workspaces to start with when there is no saved session.
    #[serde(default, rename = "workspace")]
    pub workspaces: Vec<WorkspaceSpec>,
}

fn yes() -> bool {
    true
}

fn default_theme() -> String {
    "Dark Modern".into()
}

fn default_projects() -> Vec<String> {
    ["~/scripts", "~/code", "~/src", "~/projects", "~/dev"].iter().map(|s| s.to_string()).collect()
}

impl Default for Config {
    fn default() -> Self {
        Config { font_size: default_font_size(), scrollback: default_scrollback(), status_line: false, captions: true, theme: default_theme(), projects: default_projects(), workspaces: Vec::new() }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Session {
    #[serde(default)]
    pub current: usize,
    /// The roaming terminal's folder, when it was open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roam: Option<String>,
    #[serde(default, rename = "workspace")]
    pub workspaces: Vec<WorkspaceSpec>,
}

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config")).join("jterm")
}

fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/state")).join("jterm")
}

const CONFIG: &str = include_str!("../assets/config.toml");

pub fn load_config() -> Config {
    let path = config_dir().join("config.toml");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            // The first run writes the file, every setting shown and
            // commented out.
            let _ = std::fs::create_dir_all(config_dir());
            let _ = std::fs::write(&path, CONFIG);
            return Config::default();
        }
    };
    toml::from_str(&text).unwrap_or_else(|e| {
        eprintln!("jterm: {} not read: {e}", path.display());
        Config::default()
    })
}

pub fn load_session() -> Option<Session> {
    let text = std::fs::read_to_string(state_dir().join("session.toml")).ok()?;
    toml::from_str::<Session>(&text).ok().filter(|s| !s.workspaces.is_empty())
}

pub fn save_session(session: &Session) {
    let Ok(text) = toml::to_string(session) else { return };
    let dir = state_dir();
    let _ = std::fs::create_dir_all(&dir);
    let tmp = dir.join("session.toml.tmp");
    if std::fs::write(&tmp, format!("# Saved by jterm as you work: the workspaces as you left them.\n\n{text}")).is_ok() {
        let _ = std::fs::rename(tmp, dir.join("session.toml"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_layouts() {
        let s: Config = toml::from_str(
            r#"
            [[workspace]]
            name = "jterm"
            cwd = "~/scripts/jterm"
            layout = { v = [ { h = ["claude", "claude"] }, "", { cmd = "htop", cwd = "/tmp" } ], sizes = [2, 1, 1] }

            [[workspace]]
            panes = ["claude", ""]
            split = "h"
            "#,
        )
        .unwrap();
        let mut n = 0;
        let node = s.workspaces[0].build(&mut |_| {
            n += 1;
            n
        });
        let mut ids = Vec::new();
        node.panes(&mut ids);
        assert_eq!(ids, vec![1, 2, 3, 4]);
        let Node::Split { dir: Dir::Across, sizes, .. } = &node else { panic!("{node:?}") };
        assert!((sizes[0] - 0.5).abs() < 1e-6);
        let node = s.workspaces[1].build(&mut |_| 9);
        assert!(matches!(node, Node::Split { dir: Dir::Down, .. }));
        // And back.
        let spec = spec(&node, &|_| ("claude".into(), None));
        let text = toml::to_string(&WorkspaceSpec { name: "x".into(), cwd: None, panes: vec![], split: None, layout: Some(spec) }).unwrap();
        let back: WorkspaceSpec = toml::from_str(&text).unwrap();
        assert!(matches!(back.layout, Some(Spec::Down { ref h, .. }) if h.len() == 2), "{text}");
    }
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// Sets one top-level setting in the config file, keeping the rest of it
/// as it is written, comments and all: the line that has it, or has it
/// commented out, is replaced; else it goes in above the first table.
pub fn set_config_value(key: &str, value: &str) {
    let path = config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| CONFIG.to_string());
    let setting = format!("{key} = {value}");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let tables = lines.iter().position(|l| l.trim_start().starts_with('[')).unwrap_or(lines.len());
    let is_key = |l: &str| {
        let l = l.trim_start();
        l.strip_prefix(key).is_some_and(|rest| rest.trim_start().starts_with('='))
    };
    let commented = |l: &str| l.trim_start().strip_prefix('#').is_some_and(|rest| is_key(rest.trim_start()));
    if let Some(i) = lines[..tables].iter().position(|l| is_key(l)) {
        lines[i] = setting;
    } else if let Some(i) = lines[..tables].iter().position(|l| commented(l)) {
        lines[i] = setting;
    } else {
        lines.insert(tables, setting);
    }
    let _ = std::fs::create_dir_all(config_dir());
    let tmp = path.with_extension("toml.tmp");
    if std::fs::write(&tmp, lines.join("\n") + "\n").is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}
