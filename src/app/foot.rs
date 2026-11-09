//! The project bar along the window's foot. At its left the workspace you
//! are in: a square of its color with its number, its name, the folder of
//! the pane you are in. Then the workspaces of one machine, a dot each (the
//! one you are in ringed): those of this window on it, and those its
//! session holds that no window shows, opened here when clicked, laid out
//! as they were. At the right that machine's name, which opens a list of
//! the machines, to show another's workspaces instead; and the clock.
//!
//! One machine's set at a time: Alt and a number count in it.

use super::*;
use crate::canvas::icons;

/// What a dot on the project bar stands for.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum FootDot {
    /// A workspace of this window.
    Here(usize),
    /// One a machine's session holds that no window shows: the machine,
    /// and its place in what that session last listed.
    There(usize, usize),
}

/// The folders of a layout's panes, in order.
fn cwds_in(spec: &persist::Spec, out: &mut Vec<String>) {
    match spec {
        persist::Spec::Pane { cwd: Some(c), .. } => out.push(c.clone()),
        persist::Spec::Across { v, .. } => v.iter().for_each(|s| cwds_in(s, out)),
        persist::Spec::Down { h, .. } => h.iter().for_each(|s| cwds_in(s, out)),
        _ => {}
    }
}

/// A folder as a project: its last part, "~" at home.
fn project(cwd: &str) -> String {
    let short = persist::shorten(std::path::Path::new(cwd));
    if short == "~" {
        return short;
    }
    short.rsplit('/').find(|p| !p.is_empty()).unwrap_or(&short).to_string()
}

impl App {
    /// The machine a workspace is on: its home, where it was made (its
    /// session shares it), whatever machines its terminals are on.
    pub(super) fn ws_machine(&self, wi: usize) -> usize {
        self.workspaces.get(wi).map_or(0, |ws| ws.home)
    }

    /// The machine whose workspaces the bar shows: the one the workspace
    /// you are in is on.
    pub(super) fn foot_machine(&self) -> usize {
        self.ws_machine(self.current)
    }

    /// Machine `k`'s workspaces as the bar shows them, each with its name
    /// and color: this window's on it, in their order; then those its
    /// session holds that no window shows, colored as their own say or, as
    /// any workspace is, by name.
    pub(super) fn machine_set(&self, k: usize) -> Vec<(FootDot, String, u32)> {
        let accents = self.ws_accents();
        let mut out = Vec::new();
        for wi in 0..self.workspaces.len() {
            if self.ws_machine(wi) == k {
                out.push((FootDot::Here(wi), self.workspaces[wi].name.clone(), accents.get(wi).map_or(self.accent, |a| a.0)));
            }
        }
        let Some(m) = self.machines.get(k) else { return out };
        let held: Vec<(usize, WorkspaceSpec)> = m
            .listing
            .iter()
            .enumerate()
            .filter(|(_, l)| l.label.is_empty() && !l.elsewhere)
            .filter_map(|(i, l)| toml::from_str::<WorkspaceSpec>(&l.spec).ok().map(|s| (i, s)))
            .collect();
        if !held.is_empty() {
            let mut named: Vec<(&str, Option<&str>)> = self.workspaces.iter().map(|w| (w.name.as_str(), w.accent.as_deref())).collect();
            let from = named.len();
            named.extend(held.iter().map(|(_, s)| (s.name.as_str(), s.accent.as_deref())));
            let colors = crate::theme::workspace_accents(&named, self.accent, self.config.workspace_colors);
            for (j, (i, spec)) in held.iter().enumerate() {
                let name = if spec.name.is_empty() { "workspace".to_string() } else { spec.name.clone() };
                out.push((FootDot::There(k, *i), name, colors.get(from + j).map_or(self.accent, |c| c.0)));
            }
        }
        out
    }

    /// A dot's project: its workspace's (see ws_project); for one its
    /// session holds that no window shows, the one most of its saved
    /// folders are in, on this machine.
    pub(super) fn dot_project(&self, d: FootDot) -> Option<super::projects::Project> {
        match d {
            FootDot::Here(wi) => self.ws_project(wi),
            FootDot::There(k, i) if k == 0 => {
                let spec = toml::from_str::<WorkspaceSpec>(&self.machines.get(k)?.listing.get(i)?.spec).ok()?;
                let mut cwds = Vec::new();
                cwds_in(spec.layout.as_ref()?, &mut cwds);
                super::projects::main_project(cwds.iter().filter_map(|c| super::projects::project_in(&persist::expand(c))))
            }
            FootDot::There(..) => None,
        }
    }

    /// A dot gone to: a workspace of this window shown, one held on a
    /// machine opened here.
    pub(super) fn go_dot(&mut self, d: FootDot) {
        match d {
            FootDot::Here(wi) => self.go_to(wi),
            FootDot::There(k, i) => self.open_listed(k, i),
        }
    }

    /// Alt and a number: the nth workspace of the machine the bar shows.
    pub(super) fn go_nth(&mut self, n: usize) {
        // The quake window: the nth machine's, its one workspace there.
        if crate::quake_mode() {
            if let Some(&k) = self.foot_machines().get(n) {
                self.show_machine(k);
            }
            return;
        }
        if let Some((d, ..)) = self.machine_set(self.foot_machine()).get(n) {
            self.go_dot(*d);
        }
    }

    /// One of machine `k`'s workspaces opened here as it was there: its
    /// panes asked for, the workspace made when they come.
    pub(super) fn open_listed(&mut self, k: usize, i: usize) {
        // The quake window has one workspace a machine, its own.
        if crate::quake_mode() {
            return;
        }
        let Some(l) = self.machines.get(k).and_then(|m| m.listing.get(i)).cloned() else { return };
        if !self.machines[k].client.connected() {
            let name = self.machine_name(k);
            return self.toast(&format!("{name} can't be reached"), "Its workspaces open again when it can be", None, None);
        }
        let Ok(mut spec) = toml::from_str::<WorkspaceSpec>(&l.spec) else { return };
        spec.host = (k > 0).then(|| self.machines[k].host.clone()).flatten();
        self.taking.push((k, spec, terminals::Place::Workspace));
        self.machines[k].client.take(l.panes.clone());
        // Off the list at once, not shown twice while it comes.
        self.machines[k].listing.remove(i);
        self.listed_at = Instant::now() - Duration::from_secs(60);
        self.full = true;
        self.redraw();
    }

    /// The machines the bar's list offers: this one, and each added in the
    /// sidebar or holding a workspace.
    fn foot_machines(&self) -> Vec<usize> {
        (0..self.machines.len())
            .filter(|&k| k == 0 || self.machines[k].added || self.machines[k].listing.iter().any(|l| l.label.is_empty()) || (0..self.workspaces.len()).any(|wi| self.ws_machine(wi) == k))
            .collect()
    }

    /// The bar's machine clicked: the machines, to show another's
    /// workspaces.
    pub(super) fn machine_menu(&mut self) {
        if let Some(b) = self.foot_button {
            self.machine_menu_at(b, true);
        }
    }

    /// The machines, in a menu from the button `anchor`, up from it (a bar
    /// at the foot) or down: the one shown ticked.
    pub(super) fn machine_menu_at(&mut self, anchor: Rect, up: bool) {
        use crate::palette::{Action, Item};
        let here = self.foot_machine();
        let items: Vec<Option<Item>> = self
            .foot_machines()
            .into_iter()
            .map(|k| {
                let hint = if k == here {
                    "✓".to_string()
                } else if k > 0 && !self.machines[k].client.connected() {
                    "offline".to_string()
                } else if k == 0 {
                    "this machine".to_string()
                } else {
                    String::new()
                };
                Some(Item { label: self.machine_name(k), hint, action: Action::ShowMachine(k) })
            })
            .collect();
        // Up from the button, its right edges in line, as a menu from a bar
        // at the foot opens.
        let rows = items.len() as i32;
        let pad = self.px(8.0);
        let h = rows * (self.fonts.ui.height + pad) + pad + self.px(4.0);
        let gap = self.px(4.0);
        self.open_items_menu(items);
        if let Some(m) = &mut self.menu {
            m.at = if up { (anchor.x, anchor.y - h) } else { (anchor.x, anchor.y + anchor.h + gap) };
        }
    }

    /// Machine `k`'s workspaces brought to the bar: the one you were in
    /// last there, else its first here; one its session holds, opened here,
    /// when this window has none on it; a new one there when it has none.
    /// The quake window the same: its bar lists the terminals of the
    /// machine its workspace is on.
    pub(super) fn show_machine(&mut self, k: usize) {
        if k >= self.machines.len() {
            return;
        }
        if k == self.foot_machine() {
            return;
        }
        if k > 0 && !self.machines[k].client.connected() {
            let name = self.machine_name(k);
            return self.toast(&format!("{name} can't be reached"), "Its workspaces show again when it can be", None, None);
        }
        let last = self.foot_last.get(&k).copied().filter(|&wi| wi < self.workspaces.len() && self.ws_machine(wi) == k);
        let first = (0..self.workspaces.len()).find(|&wi| self.ws_machine(wi) == k);
        if let Some(wi) = last.or(first) {
            return self.go_to(wi);
        }
        // The quake window's one there, made with a new terminal: none
        // another window has is taken up.
        let quake = crate::quake_mode();
        if !quake {
            if let Some(d) = self.machine_set(k).first().map(|(d, ..)| *d) {
                return self.go_dot(d);
            }
        }
        let host = (k > 0).then(|| self.machines[k].host.clone()).flatten();
        let name = if quake { super::quake::QUAKE_WORKSPACE.to_string() } else { String::new() };
        self.add_workspace(&WorkspaceSpec { name, host, cwd: Some("~".into()), ..Default::default() });
        self.go_to(self.workspaces.len() - 1);
    }

    /// The project bar.
    pub(super) fn draw_foot(&mut self, full: bool) {
        let h = self.foot_h();
        if h == 0 || self.workspaces.is_empty() {
            self.foot_dots.clear();
            self.foot_button = None;
            return;
        }
        let k = self.foot_machine();
        self.foot_last.insert(k, self.current);
        let set = self.machine_set(k);
        let ws = self.ws();
        let name = ws.name.clone();
        let focus = ws.focus;
        let folder = self.panes.get(&focus).and_then(|p| p.pty.cwd()).map(|c| persist::shorten(&c)).unwrap_or_default();
        let machine = self.machine_name(k);
        let accent = self.cur_accent;
        let clock = if self.config.status_line { String::new() } else { self.clock.clone() };
        let dots: Vec<(FootDot, u32)> = set.iter().map(|(d, _, c)| (*d, *c)).collect();
        // Each workspace's mark: its project's.
        let marks: Vec<Option<char>> = dots.iter().map(|(d, _)| self.dot_project(*d).map(|p| p.icon)).collect();
        let hot = self.foot_lit;
        let lit = hot == FootHot::Machine;
        let said = format!("{name}\0{folder}\0{machine}\0{accent}\0{dots:?}{marks:?}\0{}\0{clock}\0{}\0{}\0{hot:?}", self.current, self.canvas.width, self.cur_theme.bg);
        if !full && said == self.foot_said {
            return;
        }
        self.foot_said = said;
        let bar = Rect { x: 0, y: self.canvas.height - self.status_h() - h, w: self.canvas.width, h };
        let c = self.cur_theme.chrome();
        self.canvas.fill(bar, c.side);
        self.canvas.fill(Rect { x: bar.x, y: bar.y, w: bar.w, h: 1 }, c.edge);
        let ty = bar.y + (h - self.fonts.ui.height) / 2;
        let pad = self.px(10.0);
        let gap = self.px(12.0);
        // The right: the clock; the machine, a button opening the others.
        let mut right = bar.x + bar.w - pad;
        if !clock.is_empty() {
            right -= self.fonts.ui.width(&clock);
            self.canvas.text(&mut self.fonts.ui, &clock, right as f32, ty, c.dim, bar);
            right -= gap;
        }
        let ih = self.px(10.0);
        let bw = self.px(8.0) + self.fonts.ui.width(&machine) + self.px(6.0) + ih + self.px(7.0);
        let bh = (h - self.px(5.0)).max(self.px(16.0));
        let button = Rect { x: right - bw + self.px(4.0), y: bar.y + 1 + (h - 1 - bh) / 2, w: bw, h: bh };
        let lift = if self.cur_theme.dark { 0xFFFFFF } else { 0x000000 };
        self.canvas.round_fill(button, self.px(5.0), lift, if lit { 30 } else { 14 });
        let tx = (button.x + self.px(8.0)) as f32;
        self.canvas.text(&mut self.fonts.ui, &machine, tx, ty, if lit { c.text } else { mix(c.dim, c.text, 128) }, bar);
        let ix = button.x + button.w - self.px(7.0) - ih;
        self.canvas.icon_in(icons::CHEVRON_UP, Rect { x: ix, y: button.y + (button.h - ih) / 2, w: ih, h: ih }, ih, c.dim);
        self.foot_button = Some(button);
        right = button.x - gap;
        // The tile: a square of the workspace's color at the corner, its
        // number in its machine's set in it (Alt and the number go there;
        // one day the icon its settings give it). Dark or light, whichever
        // reads better on the color, as WCAG reckons it.
        // The workspace you are in, always at the far left: its mark on a
        // tile of its color (its project's mark; a dot when it has none).
        let place = dots.iter().position(|(d, _)| *d == FootDot::Here(self.current)).unwrap_or(0);
        let ts = h - self.px(4.0);
        let mark = marks.get(place).copied().flatten();
        self.mark_tile(Rect { x: bar.x + self.px(6.0), y: bar.y + 1 + (h - 1 - ts) / 2, w: ts, h: ts }, accent, mark, true);
        let tile = Rect { x: bar.x, y: bar.y, w: ts + self.px(6.0), h };
        // Its name, plain; the folder of the pane you are in, dim.
        let clip = Rect { x: tile.x + tile.w, y: bar.y, w: (right - self.px(60.0) - tile.x - tile.w).max(0), h };
        // Under the pointer (double-clicked, it is renamed): a soft lift.
        if hot == FootHot::Name && self.inline.is_none() {
            let nw = self.fonts.ui.width(&name).min((clip.w - 2 * pad).max(0));
            let lift = if self.cur_theme.dark { 0xFFFFFF } else { 0x000000 };
            self.canvas.round_fill(Rect { x: clip.x + pad - self.px(6.0), y: bar.y + self.px(3.0), w: nw + self.px(12.0), h: h - self.px(6.0) }, self.px(5.0), lift, 16);
        }
        // Being renamed: the field where its name is (wide enough to type in).
        let ws_id = self.ws().id;
        let room = (self.fonts.ui.width(&name) + self.px(80.0)).min((clip.w - 2 * pad).max(0));
        let mut end = match self.draw_inline(super::inline::Target::Workspace(ws_id), clip.x + pad, ty, room, false, clip) {
            Some(_) => clip.x + pad + room + self.px(4.0),
            None => self.canvas.text_fit(&mut self.fonts.ui, &name, clip.x + pad, ty, (clip.w - pad).max(0), c.text, clip),
        };
        // Its name, double-clicked to rename it.
        self.foot_name = Some(Rect { x: clip.x, y: bar.y, w: (end - clip.x).max(0), h });
        if !folder.is_empty() {
            let x = end + self.px(10.0);
            end = self.canvas.text_fit(&mut self.fonts.ui, &folder, x, ty, (clip.x + clip.w - x).max(0), c.dim, clip);
        }
        // The machine's workspaces, a dot each in order, all one size: the
        // one you are in ringed in its color, and nothing else.
        self.foot_dots.clear();
        // Every workspace of the machine in order, each its tile; the one
        // you are in bigger, in its place (the order never moves).
        let step = self.px(21.0);
        let (small_tile, big_tile) = (self.px(14.0), self.px(18.0));
        let mut x = end.min(clip.x + clip.w) + self.px(10.0);
        for (i, (d, color)) in dots.into_iter().enumerate() {
            if x + step > right {
                break;
            }
            // Under the pointer: a soft lift behind it.
            if hot == FootHot::Dot(i) {
                let lift = if self.cur_theme.dark { 0xFFFFFF } else { 0x000000 };
                self.canvas.round_fill(Rect { x: x + 1, y: bar.y + self.px(3.0), w: step - 2, h: h - self.px(6.0) }, self.px(5.0), lift, 22);
            }
            let here = d == FootDot::Here(self.current);
            let s = if here { big_tile } else { small_tile };
            let r = Rect { x: x + (step - s) / 2, y: bar.y + 1 + (h - 1 - s) / 2, w: s, h: s };
            self.mark_tile(r, color, marks.get(i).copied().flatten(), here);
            self.foot_dots.push((Rect { x, y: bar.y, w: step, h }, d));
            x += step;
        }
        // After them a +: a new workspace, as Ctrl N (not on the quake
        // window's bar, which has one).
        self.foot_plus = None;
        if !crate::quake_mode() && x + step <= right {
            let box_ = Rect { x: x + 1, y: bar.y + self.px(3.0), w: step - 2, h: h - self.px(6.0) };
            if hot == FootHot::Plus {
                let lift = if self.cur_theme.dark { 0xFFFFFF } else { 0x000000 };
                self.canvas.round_fill(box_, self.px(5.0), lift, 22);
            }
            let ih = self.px(10.0);
            let at = Rect { x: x + (step - ih) / 2, y: bar.y + 1 + (h - 1 - ih) / 2, w: ih, h: ih };
            self.canvas.icon_in(icons::PLUS, at, ih, if hot == FootHot::Plus { c.text } else { c.dim });
            self.foot_plus = Some(Rect { x, y: bar.y, w: step, h });
        }
        self.damage.push(bar);
    }

    /// What the tooltip says over a dot on the project bar: its number in
    /// the set, its name, and the projects its panes are in.
    pub(super) fn foot_dot_words(&self, d: FootDot) -> String {
        let set = self.machine_set(self.foot_machine());
        let n = set.iter().position(|(x, ..)| *x == d).map_or(0, |p| p + 1);
        let name = set.iter().find(|(x, ..)| *x == d).map(|(_, n, _)| n.clone()).unwrap_or_default();
        let mut cwds: Vec<String> = Vec::new();
        match d {
            FootDot::Here(wi) => {
                let mut ids = Vec::new();
                if let Some(ws) = self.workspaces.get(wi) {
                    ws.root.panes(&mut ids);
                }
                cwds.extend(ids.iter().filter_map(|id| self.panes.get(id).and_then(|p| p.pty.cwd())).map(|c| c.display().to_string()));
            }
            FootDot::There(k, i) => {
                let spec = self.machines.get(k).and_then(|m| m.listing.get(i)).and_then(|l| toml::from_str::<WorkspaceSpec>(&l.spec).ok());
                if let Some(layout) = spec.as_ref().and_then(|s| s.layout.as_ref()) {
                    cwds_in(layout, &mut cwds);
                }
            }
        }
        let mut projects: Vec<String> = Vec::new();
        for c in cwds {
            let p = project(&c);
            if !projects.contains(&p) {
                projects.push(p);
            }
        }
        if projects.is_empty() {
            format!("{n} {name}")
        } else {
            format!("{n} {name}  ·  {}", projects.join(", "))
        }
    }

    /// Whether a point is on the project bar.
    pub(super) fn foot_at(&self, _x: i32, y: i32) -> bool {
        let h = self.foot_h();
        let top = self.canvas.height - self.status_h() - h;
        h > 0 && (top..top + h).contains(&y)
    }

    /// A left click on the project bar's dots or its machine: handled.
    pub(super) fn foot_click(&mut self) -> bool {
        let (x, y) = self.pointer;
        if let Some(&(_, d)) = self.foot_dots.iter().find(|(r, _)| r.contains(x, y)) {
            self.go_dot(d);
            return true;
        }
        if self.foot_button.is_some_and(|r| r.contains(x, y)) {
            self.machine_menu();
            return true;
        }
        // The + after the dots: a new workspace, as Ctrl N.
        if self.foot_plus.is_some_and(|r| r.contains(x, y)) {
            self.new_workspace();
            return true;
        }
        // Its name, double-clicked: renamed there.
        if self.foot_name.is_some_and(|r| r.contains(x, y)) && !self.workspaces.is_empty() {
            let id = self.ws().id;
            self.inline_click(super::inline::Target::Workspace(id));
            return true;
        }
        false
    }

    /// The pointer over the bar: what it is on lit (the machine's button, a
    /// dot, the name), the bar drawn again when that changes; the hand over
    /// what a click does something to, the arrow elsewhere on it (not the
    /// text cursor of the pane behind). Whether it is on the bar.
    pub(super) fn foot_hover(&mut self) -> bool {
        let (x, y) = self.pointer;
        let on = self.foot_at(x, y);
        let hot = if !on {
            FootHot::None
        } else if self.foot_button.is_some_and(|r| r.contains(x, y)) {
            FootHot::Machine
        } else if let Some(i) = self.foot_dots.iter().position(|(r, _)| r.contains(x, y)) {
            FootHot::Dot(i)
        } else if self.foot_plus.is_some_and(|r| r.contains(x, y)) {
            FootHot::Plus
        } else if self.foot_name.is_some_and(|r| r.contains(x, y)) {
            FootHot::Name
        } else {
            FootHot::None
        };
        if hot != self.foot_lit {
            self.foot_lit = hot;
            self.redraw();
        }
        if on {
            if let Some(w) = &self.window {
                w.set_cursor(if matches!(hot, FootHot::Machine | FootHot::Dot(_) | FootHot::Plus) { CursorIcon::Pointer } else { CursorIcon::Default });
            }
        }
        on
    }

    /// The project bar right clicked: what can be done to the workspace it
    /// names.
    pub(super) fn foot_menu(&mut self) -> bool {
        let (x, y) = self.pointer;
        if !self.foot_at(x, y) || self.workspaces.is_empty() {
            return false;
        }
        use crate::palette::{Action, Item};
        let c = |label: &str, hint: &str, action| Some(Item { label: label.into(), hint: hint.into(), action });
        let chosen = self.ws().accent.clone();
        let mark = |name: Option<&str>| if chosen.as_deref().map(|c| c.to_ascii_lowercase()) == name.map(str::to_string) { "✓" } else { "" };
        let mut colors = vec![c("Automatic", mark(None), Action::WsAccent(None)), None];
        colors.extend(crate::theme::ACCENTS.iter().map(|(n, _)| {
            let mut label = n.to_string();
            label[..1].make_ascii_uppercase();
            c(&label, mark(Some(n)), Action::WsAccent(Some(n.to_string())))
        }));
        let items = vec![
            c("Rename workspace…", "", Action::Rename),
            c("Color", "›", Action::Submenu(0)),
            None,
            c("New workspace", "Ctrl N", Action::NewWorkspace),
            None,
            c("Settings", "Ctrl ,", Action::Settings),
        ];
        self.palette = None;
        self.menu_at = self.pointer;
        self.menu = Some(overlay::Menu { items, at: self.pointer, hover: None, children: vec![colors], open: None });
        self.full = true;
        self.redraw();
        true
    }
}

/// What on the project bar the pointer is over.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum FootHot {
    None,
    /// The machine's button.
    Machine,
    /// The workspace's name (double-clicked to rename).
    Name,
    /// A workspace's dot, by its place.
    Dot(usize),
    /// The + after them: a new workspace.
    Plus,
}
