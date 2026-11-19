//! Workspaces kept as saved, a change to one a draft until Save (option A
//! on the "jot: pane header badge" canvas, Workspaces page). A workspace's
//! saved layout is its terminals and how they are split; how it is now may
//! differ (a terminal clicked into a pane from the sidebar, a split, a pane
//! closed): a draft, said in its sidebar row with a dot, Revert and Save.
//! Save makes the layout as it is the saved one; Revert puts the saved one
//! back, the terminals that left it taken back from the session (they ran
//! on all the while: a pane gone is only a view gone). Gone from with a
//! draft (another workspace gone to), it is put back as saved; and after a
//! restart each opens as saved. Its name, mark and colors are not part of
//! it: they change at once.

use super::*;
use crate::mux::{app_id, base_id, split_id};

/// A layout's shape, as drafts compare them: each terminal by its base
/// id (whatever view shows it), each split by its way; sizes are not part
/// of it, being each window's own.
pub(super) fn node_shape(n: &Node) -> String {
    match n {
        Node::Pane(id) => format!("p{:x}", base_id(*id)),
        Node::Split { dir, children, .. } => format!("{}({})", if matches!(dir, Dir::Across) { "a" } else { "d" }, children.iter().map(node_shape).collect::<Vec<_>>().join(",")),
    }
}

/// The layout with each pane as its terminal's base id.
pub(super) fn base_node(n: &Node) -> Node {
    match n {
        Node::Pane(id) => Node::Pane(base_id(*id)),
        Node::Split { dir, children, sizes } => Node::Split { dir: *dir, children: children.iter().map(base_node).collect(), sizes: sizes.clone() },
    }
}

impl App {
    /// Whether workspace `wi` is not as it was saved.
    pub(super) fn is_draft(&self, wi: usize) -> bool {
        let Some(ws) = self.workspaces.get(wi) else { return false };
        ws.saved.as_ref().is_some_and(|s| node_shape(s) != node_shape(&ws.root))
    }

    /// Workspace `wi` saved as it is now.
    pub(super) fn save_workspace(&mut self, wi: usize) {
        let Some(ws) = self.workspaces.get_mut(wi) else { return };
        ws.saved = Some(base_node(&ws.root));
        crate::debuglog::log(format!("workspace {}: saved", ws.name));
        self.unsaved = true;
        self.side_dirty = true;
        self.full = true;
        self.redraw();
    }

    /// A workspace's saved layout as a spec says it, for machine `k`'s
    /// session: each terminal on the machine its host names (`k`'s when
    /// none). None when it has no terminal.
    pub(super) fn saved_from_spec(&mut self, spec: &persist::Spec, k: usize) -> Option<Node> {
        match spec {
            persist::Spec::Pane { id: Some(sid), host, empty: false, .. } => {
                let m = match host {
                    None => k,
                    Some(h) => self.machine_for(h),
                };
                Some(Node::Pane(app_id(m, *sid)))
            }
            persist::Spec::Across { v: kids, .. } | persist::Spec::Down { h: kids, .. } => {
                let dir = if matches!(spec, persist::Spec::Across { .. }) { Dir::Across } else { Dir::Down };
                let mut children: Vec<Node> = kids.iter().filter_map(|s| self.saved_from_spec(s, k)).collect();
                match children.len() {
                    0 => None,
                    1 => children.pop(),
                    n => Some(Node::Split { dir, children, sizes: vec![1.0 / n as f32; n] }),
                }
            }
            _ => None,
        }
    }

    /// Workspace `wi` put back as it was saved. The terminals that left it
    /// and that no pane here shows are taken back from their sessions
    /// first (it is put back once they come); one ended since, left out.
    pub(super) fn revert_workspace(&mut self, wi: usize) {
        if !self.is_draft(wi) {
            return;
        }
        let ws_id = self.workspaces[wi].id;
        let Some(saved) = self.workspaces[wi].saved.clone() else { return };
        let mut ids = Vec::new();
        saved.panes(&mut ids);
        let free = self.free_terminals();
        let mut want: std::collections::BTreeMap<usize, Vec<u64>> = Default::default();
        for id in ids {
            let base = base_id(id);
            if !self.views(base).is_empty() || self.held.contains_key(&base) {
                continue;
            }
            let (m, sid) = split_id(base);
            let asked = self.taking.iter().any(|(tm, s, _)| *tm == m && s.layout.as_ref().is_some_and(|l| persist::ids_in_order(l).contains(&sid)));
            let there = free.iter().any(|f| f.k == m && f.sid == sid && !f.elsewhere);
            let reached = self.machines.get(m).is_some_and(|x| x.client.connected());
            if there && reached && !asked {
                want.entry(m).or_default().push(sid);
            }
        }
        if !want.is_empty() {
            crate::debuglog::log(format!("workspace {}: reverting, taking back {want:?}", self.workspaces[wi].name));
            for (m, sids) in want {
                let pane = |sid: u64| persist::Spec::Pane { cmd: String::new(), cwd: None, pins: Vec::new(), empty: false, id: Some(sid), host: None, key: None };
                let layout = persist::Spec::Across { v: sids.iter().map(|s| pane(*s)).collect(), sizes: Vec::new() };
                self.taking.push((m, WorkspaceSpec { layout: Some(layout), ..Default::default() }, terminals::Place::Revert(ws_id)));
                self.machines[m].client.take(sids);
            }
            return;
        }
        self.put_back_saved(wi);
    }

    /// The terminals a revert waited for come (see revert_workspace): the
    /// workspace put back now, if it is still a draft.
    pub(super) fn revert_taken(&mut self, ws_id: u64) {
        if let Some(wi) = self.workspaces.iter().position(|w| w.id == ws_id) {
            if self.taking.iter().any(|(_, _, p)| *p == terminals::Place::Revert(ws_id)) {
                return;
            }
            self.put_back_saved(wi);
        }
    }

    /// Workspace `wi`'s panes made again as saved: each terminal shown by a
    /// pane it has now, else by a new view of it (one the session gave);
    /// one neither, left out. The panes it shows no more, let go.
    fn put_back_saved(&mut self, wi: usize) {
        let Some(saved) = self.workspaces[wi].saved.clone() else { return };
        let mut was = Vec::new();
        self.workspaces[wi].root.panes(&mut was);
        let mut used = self.shown_ids();
        used.retain(|id| !was.contains(id));
        let Some(root) = self.node_from_saved(&saved, &mut used) else {
            crate::debuglog::log(format!("workspace {}: nothing of it as saved is left", self.workspaces[wi].name));
            return;
        };
        let mut now = Vec::new();
        root.panes(&mut now);
        crate::debuglog::log(format!("workspace {}: put back as saved, {} panes", self.workspaces[wi].name, now.len()));
        let ws = &mut self.workspaces[wi];
        ws.root = root;
        // As saved it is; a terminal that ended is left out of that too.
        ws.saved = Some(base_node(&ws.root));
        if !now.contains(&ws.focus) {
            ws.focus = now[0];
        }
        if ws.zoom.is_some_and(|z| !now.contains(&z)) {
            ws.zoom = None;
        }
        ws.spaces.retain(|s| now.contains(s));
        ws.minimized.retain(|m| now.contains(m));
        ws.left_out.retain(|m| now.contains(m));
        for id in was.into_iter().filter(|id| !now.contains(id)) {
            if !self.shown_ids().contains(&id) && self.roam != Some(id) {
                self.let_go(id);
            }
        }
        self.replay_held();
        self.full = true;
        self.changed();
    }

    /// A saved layout's panes: each terminal by a view here not `used`
    /// already, else a new view of one the session gave (held).
    fn node_from_saved(&mut self, node: &Node, used: &mut Vec<PaneId>) -> Option<Node> {
        match node {
            Node::Pane(base) => {
                if let Some(v) = self.views(*base).into_iter().find(|v| !used.contains(v)) {
                    used.push(v);
                    return Some(Node::Pane(v));
                }
                if self.held.contains_key(base) {
                    let (m, sid) = split_id(*base);
                    self.target = m;
                    let v = self.spawn(Start { cmd: String::new(), cwd: pty::home(), pane: Some(sid), host: None, key: None });
                    used.push(v);
                    return Some(Node::Pane(v));
                }
                None
            }
            Node::Split { dir, children, sizes } => {
                let mut kids = Vec::new();
                let mut shares = Vec::new();
                for (i, c) in children.iter().enumerate() {
                    if let Some(n) = self.node_from_saved(c, used) {
                        kids.push(n);
                        shares.push(sizes.get(i).copied().filter(|s| *s > 0.0).unwrap_or(1.0));
                    }
                }
                match kids.len() {
                    0 => None,
                    1 => kids.pop(),
                    _ => {
                        let total: f32 = shares.iter().sum();
                        Some(Node::Split { dir: *dir, children: kids, sizes: shares.iter().map(|s| s / total).collect() })
                    }
                }
            }
        }
    }

    /// A new workspace with workspace `wi`'s terminals, laid out as it is
    /// now (`as_is`: Save as new workspace, after which `wi` goes back to
    /// as it was saved) or as it was saved (Duplicate). The terminals are
    /// shown in both: second views, which the session gives.
    pub(super) fn copy_workspace(&mut self, wi: usize, as_is: bool) {
        let k = self.ws_machine(wi);
        let spec = self.ws_spec(wi, k);
        let layout = if as_is { spec.layout.clone() } else { spec.saved.clone().or(spec.layout.clone()) };
        let Some(layout) = layout else { return };
        let name = format!("{}-{}", self.workspaces[wi].name, if as_is { "new" } else { "copy" });
        let mine: Vec<u64> = persist::panes_in_order(&layout).into_iter().filter(|(h, _)| h.is_none()).map(|(_, s)| s).collect();
        if mine.is_empty() || !self.machines.get(k).is_some_and(|m| m.client.connected()) {
            return;
        }
        let host = (k > 0).then(|| self.machines[k].host.clone()).flatten();
        let new = WorkspaceSpec { name, host, layout: Some(layout), accent: spec.accent.clone(), theme: spec.theme.clone(), ..Default::default() };
        crate::debuglog::log(format!("workspace {}: {}", self.workspaces[wi].name, if as_is { "saved as new" } else { "duplicated" }));
        self.taking.push((k, new, terminals::Place::Workspace));
        self.machines[k].client.take(mine);
    }

    /// Workspace `wi` moved one place up (-1) or down (1) among its
    /// machine's.
    pub(super) fn move_workspace(&mut self, wi: usize, by: i32) {
        let k = self.ws_machine(wi);
        let same: Vec<usize> = (0..self.workspaces.len()).filter(|&i| self.ws_machine(i) == k).collect();
        let Some(at) = same.iter().position(|&i| i == wi) else { return };
        let to = at as i32 + by;
        if to < 0 || to as usize >= same.len() {
            return;
        }
        let other = same[to as usize];
        let showing = self.workspaces.get(self.current).map(|w| w.id);
        self.workspaces.swap(wi, other);
        if let Some(c) = showing.and_then(|id| self.workspaces.iter().position(|w| w.id == id)) {
            self.current = c;
        }
        self.side_dirty = true;
        self.changed();
    }

    /// Workspace `wi` gone: its panes closed (their terminals run on), as
    /// Close workspace does.
    pub(super) fn delete_workspace(&mut self, wi: usize, event_loop: &ActiveEventLoop) {
        if wi >= self.workspaces.len() {
            return;
        }
        if wi != self.current {
            self.go_to(wi);
        }
        // Gone from the list now, not put back as saved on the way out.
        self.ws_mut().saved = None;
        let mut ids = Vec::new();
        self.ws().root.panes(&mut ids);
        for id in ids {
            self.close(id, event_loop);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(dir: Dir, children: Vec<Node>) -> Node {
        let n = children.len();
        Node::Split { dir, children, sizes: vec![1.0 / n as f32; n] }
    }

    #[test]
    fn a_layout_s_shape_is_its_terminals_and_splits_not_its_sizes_nor_views() {
        let a = split(Dir::Across, vec![Node::Pane(1), Node::Pane(2)]);
        let mut b = split(Dir::Across, vec![Node::Pane(1), Node::Pane((3 << 56) | 2)]);
        if let Node::Split { sizes, .. } = &mut b {
            *sizes = vec![0.8, 0.2];
        }
        assert_eq!(node_shape(&a), node_shape(&b));
        let c = split(Dir::Down, vec![Node::Pane(1), Node::Pane(2)]);
        assert_ne!(node_shape(&a), node_shape(&c));
        let d = split(Dir::Across, vec![Node::Pane(1), Node::Pane(4)]);
        assert_ne!(node_shape(&a), node_shape(&d));
    }

    #[test]
    fn a_saved_layout_is_kept_by_terminal_not_by_view() {
        let n = split(Dir::Across, vec![Node::Pane((2 << 56) | 7), Node::Pane(9)]);
        assert_eq!(node_shape(&base_node(&n)), "a(p7,p9)");
    }
}
