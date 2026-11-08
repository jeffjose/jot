//! What is kept between runs. Settings and workspaces you write yourself
//! live in ~/.config/jot/config.toml; the workspaces as you left them
//! are saved as you go to ~/.local/state/jot/session.toml, and brought
//! back when jot starts: each pane's command, started afresh in the
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
        /// Commands pinned to the top of the pane.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pins: Vec<String>,
        /// Empty space, not a terminal.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        empty: bool,
        /// Its pane in the running session, to be taken up again.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<u64>,
        /// The machine it runs on, if not the one whose session this is.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
    },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WorkspaceSpec {
    #[serde(default)]
    pub name: String,
    /// The machine its panes are on, reached by ssh; none for the machine
    /// whose session this is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The shorthand: panes in a row, `split` deciding which way.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub panes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<Dir>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<Spec>,
    /// Its own accent and theme, over the ones in the settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// Commands it runs when it opens, and each time it is gone to.
    #[serde(default, rename = "on-open", skip_serializing_if = "Option::is_none")]
    pub on_open: Option<String>,
    #[serde(default, rename = "on-focus", skip_serializing_if = "Option::is_none")]
    pub on_focus: Option<String>,
}

/// A pane to be started: its command and folder.
#[derive(Clone)]
pub struct Start {
    pub cmd: String,
    pub cwd: PathBuf,
    /// A pane the session already has, to be taken up rather than started.
    pub pane: Option<u64>,
    /// The machine to start it on, if not the window's choice.
    pub host: Option<String>,
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
        Spec::Command(cmd) => Node::Pane(start(Start { cmd: cmd.clone(), cwd: cwd.to_path_buf(), pane: None, host: None })),
        Spec::Pane { cmd, cwd: dir, id, host, .. } => {
            let cwd = dir.as_deref().map(expand).unwrap_or_else(|| cwd.to_path_buf());
            Node::Pane(start(Start { cmd: cmd.clone(), cwd, pane: *id, host: host.clone() }))
        }
        Spec::Across { v, sizes } => split(Dir::Across, v, sizes, &mut |k| build(k, cwd, start)).unwrap_or_else(|| build(&Spec::Command(String::new()), cwd, start)),
        Spec::Down { h, sizes } => split(Dir::Down, h, sizes, &mut |k| build(k, cwd, start)).unwrap_or_else(|| build(&Spec::Command(String::new()), cwd, start)),
    }
}

/// A layout as it is now, to be saved: `pane` gives each pane's command
/// and the folder it is in.
pub fn spec(node: &Node, pane: &impl Fn(PaneId) -> (String, Option<PathBuf>, Vec<String>, bool, Option<String>)) -> Spec {
    match node {
        Node::Pane(id) => {
            let (cmd, cwd, pins, empty, host) = pane(*id);
            Spec::Pane { cmd, cwd: cwd.map(|p| shorten(&p)), pins, empty, id: Some(crate::mux::split_id(*id).1), host }
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

pub fn default_font_size() -> f32 {
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
    /// A bar along the bottom in the workspace's color: its name, the folder
    /// of the pane you are in, and its machine.
    #[serde(default = "yes")]
    pub project_bar: bool,
    /// The window's title bar: "jot" (jot's own, in its interface font) or
    /// "system" (the desktop's).
    #[serde(default = "default_title_bar")]
    pub title_bar: String,
    /// The long-command editor's colors: "Terminal" (the theme's own) or
    /// one of the schemes Settings offers.
    #[serde(default = "default_editor_colors")]
    pub editor_colors: String,
    /// A caption along the top of each pane.
    #[serde(default = "yes")]
    pub captions: bool,
    /// The quake window (jot --quake), as Tilix's Quake mode: its share of
    /// the monitor's height and width, in percent; top or bottom; left,
    /// center or right; on the monitor the pointer is on, or the primary;
    /// on every workspace (always over every other window); hidden when it
    /// loses focus.
    #[serde(default = "default_quake_height")]
    pub quake_height: u32,
    #[serde(default = "default_quake_width")]
    pub quake_width: u32,
    #[serde(default = "default_quake_position")]
    pub quake_position: String,
    #[serde(default = "default_quake_align")]
    pub quake_align: String,
    #[serde(default = "default_quake_monitor")]
    pub quake_monitor: String,
    #[serde(default = "yes")]
    pub quake_all_workspaces: bool,
    #[serde(default)]
    pub quake_hide_on_blur: bool,
    /// Characters joined as the font draws them, => and != in Fira Code.
    #[serde(default = "yes")]
    pub ligatures: bool,
    /// A bar beside each command and its output, green or red by how it
    /// ended, with its time when it failed or was slow.
    #[serde(default)]
    pub command_bars: bool,
    /// Bold text drawn in the font's bold; off, in its usual weight.
    #[serde(default = "yes")]
    pub bold_text: bool,
    /// Subpixel text: "auto" as the desktop has it, "rgb", "bgr" or "none".
    #[serde(default = "default_subpixel")]
    pub subpixel: String,
    /// Workspaces without an accent of their own are each given a
    /// different one, all but the first.
    #[serde(default = "yes")]
    pub workspace_colors: bool,
    /// The color theme, by name.
    #[serde(default = "default_theme")]
    pub theme: String,
    /// The color of what is picked out: a Radix color by name, or RRGGBB.
    #[serde(default = "default_accent")]
    pub accent: String,
    /// The terminal's font, by family.
    #[serde(default = "default_font_family")]
    pub font_family: String,
    /// The font of jot's own words: captions, menus, settings.
    #[serde(default = "default_ui_font_family")]
    pub ui_font_family: String,
    /// The height of a line, as a share of the font's own: 1.1 is VS Code's.
    /// Text is made darker or lighter until it has this much contrast
    /// with its ground, as VS Code's terminal does; 1 leaves it as it is.
    #[serde(default = "default_min_contrast")]
    pub min_contrast: f32,
    #[serde(default = "default_line_height")]
    pub line_height: f32,
    #[serde(default = "default_ui_font_size")]
    pub ui_font_size: f32,
    /// Folders whose folders are projects, offered by the palette to open
    /// as workspaces.
    #[serde(default = "default_projects")]
    pub projects: Vec<String>,
    /// Slash commands of your own: "name" = "text" puts the text on the
    /// command line; "!name" = "command" runs it.
    #[serde(default)]
    pub slash: std::collections::BTreeMap<String, String>,
    /// What puts frames on the screen: "gpu" (with the CPU when there
    /// is no GPU to use), or "cpu".
    /// The last session at the start: "ask" (open fresh, with a card to
    /// bring it back), "always", or "never".
    #[serde(default = "default_restore")]
    pub restore: String,
    #[serde(default = "default_renderer")]
    pub renderer: String,
    /// Patterns looked for in what panes print, and what to do on them.
    #[serde(default, rename = "trigger")]
    pub triggers: Vec<TriggerSpec>,
    /// Workspaces to start with when there is no saved session.
    #[serde(default, rename = "workspace")]
    pub workspaces: Vec<WorkspaceSpec>,
}

pub fn yes() -> bool {
    true
}

pub fn default_accent() -> String {
    "indigo".into()
}

pub fn default_font_family() -> String {
    crate::font::TERMINAL_DEFAULT.into()
}

pub fn default_ui_font_family() -> String {
    crate::font::INTERFACE_DEFAULT.into()
}

pub fn default_min_contrast() -> f32 {
    4.5
}

pub fn default_line_height() -> f32 {
    1.1
}

pub fn default_ui_font_size() -> f32 {
    13.0
}

/// Tilix's Quake defaults.
pub fn default_quake_height() -> u32 {
    40
}

pub fn default_quake_width() -> u32 {
    100
}

pub fn default_quake_position() -> String {
    "top".into()
}

pub fn default_quake_align() -> String {
    "center".into()
}

pub fn default_quake_monitor() -> String {
    "pointer".into()
}

pub fn default_subpixel() -> String {
    "auto".into()
}

pub fn default_title_bar() -> String {
    "jot".into()
}

pub fn default_editor_colors() -> String {
    "Terminal".into()
}

pub fn default_theme() -> String {
    "Dark 2026".into()
}

fn default_projects() -> Vec<String> {
    ["~/scripts", "~/code", "~/src", "~/projects", "~/dev"].iter().map(|s| s.to_string()).collect()
}

impl Default for Config {
    fn default() -> Self {
        Config { font_size: default_font_size(), scrollback: default_scrollback(), triggers: Vec::new(), renderer: default_renderer(), restore: default_restore(), status_line: false, project_bar: true, title_bar: default_title_bar(), editor_colors: default_editor_colors(), captions: true, quake_height: default_quake_height(), quake_width: default_quake_width(), quake_position: default_quake_position(), quake_align: default_quake_align(), quake_monitor: default_quake_monitor(), quake_all_workspaces: true, quake_hide_on_blur: false, ligatures: true, command_bars: false, bold_text: true, subpixel: default_subpixel(), workspace_colors: true, theme: default_theme(), accent: default_accent(), font_family: default_font_family(), ui_font_family: default_ui_font_family(), ui_font_size: default_ui_font_size(), line_height: default_line_height(), min_contrast: default_min_contrast(), projects: default_projects(), slash: Default::default(), workspaces: Vec::new() }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Session {
    #[serde(default)]
    pub current: usize,
    /// Windows that quit with workspaces of their own, after this one: the
    /// session gives a window one's, and this many more open, each taking
    /// up the next (see App::new).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub more: u32,
    /// The roaming terminal's folder, when it was open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roam: Option<String>,
    #[serde(default, rename = "workspace")]
    pub workspaces: Vec<WorkspaceSpec>,
    /// Panes that were in windows of their own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub popouts: Vec<Spec>,
    /// Where the window was, how large, its zoom: opened so again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowState>,
    /// Each window's own, when there were more than one: its workspaces,
    /// where it was. Saved by the session for after the machine restarts,
    /// with every window's workspaces above as well (all an older jot reads).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<Session>,
}

fn config_dir() -> PathBuf {
    renamed(std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config")))
}

pub fn state_dir() -> PathBuf {
    renamed(std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/state")))
}

/// jot's folder in `base`; one left from when it was called jterm is
/// moved to it, the first time.
fn renamed(base: PathBuf) -> PathBuf {
    let dir = base.join("jot");
    let old = base.join("jterm");
    if !dir.exists() && old.is_dir() {
        let _ = std::fs::rename(&old, &dir);
    }
    dir
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
        eprintln!("jot: {} not read: {e}", path.display());
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
    if std::fs::write(&tmp, format!("# Saved by jot as you work: the workspaces as you left them.\n\n{text}")).is_ok() {
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
            name = "jot"
            cwd = "~/scripts/jot"
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
        let spec = spec(&node, &|_| ("claude".into(), None, Vec::new(), false, None));
        let text = toml::to_string(&WorkspaceSpec { name: "x".into(), cwd: None, panes: vec![], split: None, layout: Some(spec), ..Default::default() }).unwrap();
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

/// Takes a setting out of the config file, so it is as it started: its
/// line is commented out again, the rest of the file left as it is.
pub fn unset_config_value(key: &str) {
    let path = config_path();
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let tables = lines.iter().position(|l| l.trim_start().starts_with('[')).unwrap_or(lines.len());
    let mut changed = false;
    for line in lines[..tables].iter_mut() {
        let t = line.trim_start();
        if t.strip_prefix(key).is_some_and(|rest| rest.trim_start().starts_with('=')) {
            *line = format!("# {t}");
            changed = true;
        }
    }
    if changed {
        let tmp = path.with_extension("toml.tmp");
        if std::fs::write(&tmp, lines.join("\n") + "\n").is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }
}

/// A trigger in the config: a pattern, and notify (a desktop notice),
/// open (the link on the line), or run (`command`, with the line in
/// $JOT_MATCH).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TriggerSpec {
    #[serde(rename = "match")]
    pub pattern: String,
    #[serde(default = "default_action")]
    pub action: String,
    #[serde(default)]
    pub command: Option<String>,
}

fn default_restore() -> String {
    "ask".into()
}

fn default_renderer() -> String {
    "gpu".into()
}

fn default_action() -> String {
    "notify".into()
}

/// Where panes' scrollback is kept between runs.
fn scrollback_dir() -> PathBuf {
    state_dir().join("scrollback")
}

/// Each pane's scrollback, by its place (as "1-2", workspace and pane),
/// written; any kept from panes that are gone taken away.
pub fn save_scrollback(dumps: &[(String, String)]) {
    let dir = scrollback_dir();
    let _ = std::fs::create_dir_all(&dir);
    for (name, text) in dumps {
        let tmp = dir.join(format!("{name}.vt.tmp"));
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(tmp, dir.join(format!("{name}.vt")));
        }
    }
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().trim_end_matches(".vt").to_string();
            if !dumps.iter().any(|(n, _)| *n == name) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// A pane's scrollback from the last run, and when it was kept.
pub fn load_scrollback(name: &str) -> Option<(String, std::time::SystemTime)> {
    let path = scrollback_dir().join(format!("{name}.vt"));
    let when = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
    Some((std::fs::read_to_string(path).ok()?, when))
}

/// When the last session was saved.
pub fn session_saved_at() -> Option<std::time::SystemTime> {
    std::fs::metadata(state_dir().join("session.toml")).and_then(|m| m.modified()).ok()
}

/// The pins of each pane a layout makes, in the order its panes are made.
pub fn pins_in_order(spec: &Spec) -> Vec<Vec<String>> {
    match spec {
        Spec::Command(_) => vec![Vec::new()],
        Spec::Pane { pins, .. } => vec![pins.clone()],
        Spec::Across { v: kids, .. } | Spec::Down { h: kids, .. } => {
            if kids.is_empty() {
                vec![Vec::new()]
            } else {
                kids.iter().flat_map(pins_in_order).collect()
            }
        }
    }
}

/// Which panes a layout makes are empty space, in the order it makes them.
pub fn empty_in_order(spec: &Spec) -> Vec<bool> {
    match spec {
        Spec::Command(_) => vec![false],
        Spec::Pane { empty, .. } => vec![*empty],
        Spec::Across { v: kids, .. } | Spec::Down { h: kids, .. } => {
            if kids.is_empty() {
                vec![false]
            } else {
                kids.iter().flat_map(empty_in_order).collect()
            }
        }
    }
}

/// How the window's sidebar was left: shown or tucked away, and how wide.
pub fn load_sidebar() -> (bool, f32) {
    let text = std::fs::read_to_string(state_dir().join("sidebar")).unwrap_or_default();
    let mut words = text.split_whitespace();
    let open = words.next().map_or(true, |w| w != "hidden");
    let width = words.next().and_then(|w| w.parse().ok()).unwrap_or(240.0);
    (open, width)
}

pub fn save_sidebar(open: bool, width: f32) {
    let _ = std::fs::create_dir_all(state_dir());
    let _ = std::fs::write(state_dir().join("sidebar"), format!("{} {width:.0}\n", if open { "shown" } else { "hidden" }));
}

/// The session's names of a layout's panes on its own machine (those it
/// names, and on no other machine), in order.
pub fn ids_in_order(spec: &Spec) -> Vec<u64> {
    match spec {
        Spec::Command(_) => Vec::new(),
        Spec::Pane { id, host, .. } => id.filter(|_| host.is_none()).into_iter().collect(),
        Spec::Across { v: kids, .. } | Spec::Down { h: kids, .. } => kids.iter().flat_map(ids_in_order).collect(),
    }
}

/// The machines added in the sidebar, by how they are reached.
pub fn load_machines() -> Vec<String> {
    std::fs::read_to_string(state_dir().join("machines")).unwrap_or_default().lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

pub fn save_machines(hosts: &[String]) {
    let _ = std::fs::create_dir_all(state_dir());
    let _ = std::fs::write(state_dir().join("machines"), hosts.iter().map(|h| format!("{h}\n")).collect::<String>());
}

/// Names given to terminals (/rename, or the sidebar's Rename), one a line:
/// the machine (empty for this one), its terminal's name in that machine's
/// session, and the name, split by tabs.
pub fn load_term_names() -> Vec<(String, u64, String)> {
    std::fs::read_to_string(state_dir().join("names"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut f = l.splitn(3, '\t');
            let host = f.next()?.to_string();
            let sid = f.next()?.parse().ok()?;
            let name = f.next()?.to_string();
            (!name.is_empty()).then_some((host, sid, name))
        })
        .collect()
}

pub fn save_term_names(names: &[(String, u64, String)]) {
    let _ = std::fs::create_dir_all(state_dir());
    let _ = std::fs::write(state_dir().join("names"), names.iter().map(|(h, s, n)| format!("{h}\t{s}\t{}\n", n.replace(['\t', '\n'], " "))).collect::<String>());
}

/// The main window as it was left: where, how large (in pixels, its size
/// before it was maximized when it is), whether maximized, and the font
/// size Ctrl = and Ctrl - had made it; opened so again.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub maximized: bool,
    pub font_size: f32,
}

pub fn load_window() -> Option<WindowState> {
    let text = std::fs::read_to_string(state_dir().join("window")).ok()?;
    let mut w = text.split_whitespace();
    let mut next = || w.next();
    let (x, y, width, height) = (next()?.parse().ok()?, next()?.parse().ok()?, next()?.parse().ok()?, next()?.parse().ok()?);
    let maximized = next()? == "maximized";
    let font_size = next()?.parse().ok()?;
    (width > 0 && height > 0).then_some(WindowState { x, y, w: width, h: height, maximized, font_size })
}

pub fn save_window(s: &WindowState) {
    let _ = std::fs::create_dir_all(state_dir());
    let _ = std::fs::write(state_dir().join("window"), format!("{} {} {} {} {} {}\n", s.x, s.y, s.w, s.h, if s.maximized { "maximized" } else { "normal" }, s.font_size));
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// The machines whose terminals the sidebar folds away, one a line (an
/// empty line: this one).
pub fn load_folded() -> std::collections::HashSet<String> {
    std::fs::read_to_string(state_dir().join("sidebar-folded")).map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default()
}

pub fn save_folded(hosts: &std::collections::HashSet<String>) {
    let _ = std::fs::create_dir_all(state_dir());
    let mut lines: Vec<&str> = hosts.iter().map(String::as_str).collect();
    lines.sort();
    let _ = std::fs::write(state_dir().join("sidebar-folded"), lines.iter().map(|h| format!("{h}\n")).collect::<String>());
}
