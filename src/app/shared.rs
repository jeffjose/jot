//! Workspaces shared among windows. A machine's session keeps the
//! workspaces of every window on it that shares them (a regular window;
//! not the quake window, whose workspace is its own): their names, colors,
//! the terminals in them and how they are split. Each window shows them all
//! and arranges nothing on its own but the sizes of its panes, which pane
//! has the keys, and which workspace it shows.
//!
//! A window says its workspaces as it always has (its layout, see
//! App::save), each with its id and the change (rev) it is of; the session
//! takes what changed into its list and tells every window the list
//! (Output::Workspaces), with the terminals they do not have yet. Here, a
//! window takes the list in: a workspace new to it made, one changed by
//! another window made again from its terminals, one closed gone, and all
//! in the session's order.

use super::*;
use crate::mux::app_id;
use crate::persist::Spec;

/// How many shapes a window remembers having said of each workspace: one
/// of them told back by the session is its own change, not another's.
const SENT_KEPT: usize = 8;

impl App {
    /// Machine `k`'s session's shared workspaces, with the terminals in them
    /// this window did not have, now its.
    pub(super) fn sync_workspaces(&mut self, k: usize, specs: Vec<String>, held: Vec<crate::mux::Held>) {
        self.shares_with.insert(k);
        let mut given: Vec<u64> = held.iter().map(|h| h.pane).collect();
        for h in held {
            self.held.insert(app_id(k, h.pane), h);
        }
        // Given back on reconnecting, waiting for this list.
        given.extend(self.unclaimed.remove(&k).unwrap_or_default());
        let specs: Vec<WorkspaceSpec> = specs.iter().filter_map(|t| toml::from_str(t).ok()).collect();
        self.apply_shared(k, specs, &given);
    }

    /// What sync_workspaces does, again when terminals it waited for come.
    fn apply_shared(&mut self, k: usize, specs: Vec<WorkspaceSpec>, given: &[u64]) {
        if self.workspaces.is_empty() {
            return;
        }
        let ids: Vec<u64> = specs.iter().filter_map(|s| s.id).collect();
        let showing = self.workspaces.get(self.current).map(|w| w.id);
        crate::debuglog::log(format!("machine {k}: {} shared workspaces", specs.len()));
        // Closed in another window: shared once (it has a rev), on this
        // machine, and not in the list.
        let gone: Vec<usize> = (0..self.workspaces.len()).filter(|&wi| self.workspaces[wi].rev > 0 && self.ws_machine(wi) == k && !ids.contains(&self.workspaces[wi].id)).collect();
        for wi in gone.into_iter().rev() {
            self.drop_workspace(wi);
        }
        let mut missing: Vec<u64> = Vec::new();
        for spec in &specs {
            let Some(id) = spec.id else { continue };
            let rev = spec.rev.unwrap_or(0);
            match self.workspaces.iter().position(|w| w.id == id) {
                Some(wi) => {
                    // This window's own change told back: it and the ones said
                    // before it done with. Another's: what was said before is
                    // past.
                    let shape = spec.shared_shape();
                    let ours = match self.sent.get_mut(&id) {
                        Some(said) => match said.iter().position(|s| *s == shape) {
                            Some(i) => {
                                said.drain(..=i);
                                true
                            }
                            None => {
                                said.clear();
                                false
                            }
                        },
                        None => false,
                    };
                    if !ours && self.ws_spec(wi, k).shared_shape() != shape {
                        self.reshape(wi, k, spec, &mut missing);
                    }
                    self.workspaces[wi].rev = rev;
                }
                None => {
                    let before = self.workspaces.len();
                    self.make_shared(k, spec, &mut missing);
                    if self.workspaces.len() > before {
                        let wi = self.workspaces.len() - 1;
                        self.workspaces[wi].rev = rev;
                    }
                }
            }
        }
        self.order_like(k, &ids);
        let want = self.want_current.take().or(showing);
        if let Some(wi) = want.and_then(|id| self.workspaces.iter().position(|w| w.id == id)) {
            self.current = wi;
        }
        self.current = self.current.min(self.workspaces.len().saturating_sub(1));
        // Terminals given that no workspace here took up (one the window had
        // closed meanwhile): left to the session, as this window does not
        // show them.
        for p in given {
            let id = app_id(k, *p);
            if self.held.remove(&id).is_some() && self.views(id).is_empty() {
                if let Some(m) = self.machines.get(k) {
                    m.client.release(*p);
                }
            }
        }
        // A terminal shown twice here wants a view of its own, which only
        // the session can give (with what it printed): asked for, and the
        // list taken in again when it comes; meanwhile this window says
        // nothing of its workspaces to that machine, so as not to say them
        // without it.
        if missing.is_empty() {
            self.sync_waiting.remove(&k);
        } else {
            missing.sort();
            missing.dedup();
            crate::debuglog::log(format!("machine {k}: {} terminals asked for, to show twice", missing.len()));
            if let Some(m) = self.machines.get(k) {
                m.client.take(missing.clone());
            }
            self.sync_waiting.insert(k, (missing, specs));
        }
        self.full = true;
        self.changed();
    }

    /// Terminals taken (ToClient::Taken) that a list waits for: in, and the
    /// list taken in again. Given back when they are not.
    pub(super) fn shared_taken(&mut self, k: usize, held: Vec<crate::mux::Held>) -> Option<Vec<crate::mux::Held>> {
        let waited = self.sync_waiting.get(&k).is_some_and(|(wanted, _)| !held.is_empty() && held.iter().all(|h| wanted.contains(&h.pane)));
        if !waited {
            return Some(held);
        }
        let (_, specs) = self.sync_waiting.remove(&k)?;
        for h in held {
            self.held.insert(app_id(k, h.pane), h);
        }
        self.apply_shared(k, specs, &[]);
        None
    }

    /// Workspace `wi` as machine `k`'s session has it (see App::save).
    pub(super) fn ws_spec(&self, wi: usize, k: usize) -> WorkspaceSpec {
        let ws = &self.workspaces[wi];
        let describe = |id: PaneId| {
            let m = crate::mux::split_id(id).0;
            let host = (m != k).then(|| self.machines.get(m).and_then(|x| x.host.clone())).flatten();
            let empty = ws.spaces.contains(&id);
            match self.panes.get(&id) {
                Some(p) => (p.cmd.clone(), p.pty.cwd(), p.pins.iter().map(|pin| pin.cmd.clone()).collect(), empty, host),
                None => (String::new(), None, Vec::new(), empty, host),
            }
        };
        WorkspaceSpec { name: ws.name.clone(), layout: Some(persist::spec(&ws.root, &describe)), accent: ws.accent.clone(), theme: ws.theme.clone(), on_open: ws.on_open.clone(), on_focus: ws.on_focus.clone(), id: Some(ws.id), rev: Some(ws.rev), ..Default::default() }
    }

    /// Remembered as said to the session: told back, it is this window's.
    pub(super) fn said_shape(&mut self, id: u64, shape: String) {
        let kept = self.sent.entry(id).or_default();
        if kept.last() != Some(&shape) {
            kept.push(shape);
            if kept.len() > SENT_KEPT {
                kept.remove(0);
            }
        }
    }

    /// A shared workspace new to this window, made from its terminals.
    fn make_shared(&mut self, k: usize, spec: &WorkspaceSpec, missing: &mut Vec<u64>) {
        let Some(layout) = &spec.layout else { return };
        let mut used = self.shown_ids();
        let Some(root) = self.node_for(layout, k, &mut used, missing) else { return };
        let mut ids = Vec::new();
        root.panes(&mut ids);
        crate::debuglog::log(format!("shared: workspace {} made, on machine {k}, {} panes", spec.name, ids.len()));
        let id = spec.id.unwrap_or_else(new_ws_id);
        let name = if spec.name.is_empty() { sidebar::random_name() } else { spec.name.clone() };
        self.workspaces.push(Workspace { id, rev: spec.rev.unwrap_or(0), name, focus: ids[0], root, zoom: None, accent: spec.accent.clone(), theme: spec.theme.clone(), on_open: spec.on_open.clone(), on_focus: spec.on_focus.clone(), broadcast: false, left_out: Vec::new(), minimized: Vec::new(), spaces: Vec::new(), open: true });
    }

    /// Workspace `wi` made again as another window changed it: its name,
    /// colors, and its panes as the change has them, the terminals this
    /// window shows reused; focus and zoom kept where they still can be.
    /// The terminals it shows no more, let go.
    fn reshape(&mut self, wi: usize, k: usize, spec: &WorkspaceSpec, missing: &mut Vec<u64>) {
        {
            let ws = &mut self.workspaces[wi];
            ws.name = if spec.name.is_empty() { ws.name.clone() } else { spec.name.clone() };
            ws.accent = spec.accent.clone();
            ws.theme = spec.theme.clone();
        }
        let Some(layout) = &spec.layout else { return };
        let mut was = Vec::new();
        self.workspaces[wi].root.panes(&mut was);
        let mine = self.ws_spec(wi, k).layout.map(|l| persist::shape(&l));
        if mine.as_deref() == Some(persist::shape(layout).as_str()) {
            return;
        }
        // Those shown in its other workspaces (and pop-outs) are not this
        // one's to reuse; its own are.
        let mut used = self.shown_ids();
        used.retain(|id| !was.contains(id));
        let Some(root) = self.node_for(layout, k, &mut used, missing) else { return };
        let mut now = Vec::new();
        root.panes(&mut now);
        let ws = &mut self.workspaces[wi];
        ws.root = root;
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
            self.let_go_shown(id);
        }
    }

    /// A pane here no workspace shows any more: its terminal, still running
    /// and shown nowhere else here, left to the session (its last view
    /// gone, the session is told so; see Pty's drop).
    fn let_go_shown(&mut self, id: PaneId) {
        if self.shown_ids().contains(&id) || self.roam == Some(id) {
            return;
        }
        self.let_go(id);
    }

    /// Workspace `wi` gone here, closed in another window: its terminals
    /// left to the session (ended there, or shown in its other workspaces
    /// here).
    fn drop_workspace(&mut self, wi: usize) {
        crate::debuglog::log(format!("shared: workspace {} closed elsewhere", self.workspaces[wi].name));
        let ws = self.workspaces.remove(wi);
        let mut ids = Vec::new();
        ws.root.panes(&mut ids);
        for id in ids {
            self.let_go_shown(id);
        }
        if self.current > wi || self.current >= self.workspaces.len() {
            self.current = self.current.saturating_sub(1);
        }
        // None left: a new one, as a window always has one.
        if self.workspaces.is_empty() {
            self.add_workspace(&WorkspaceSpec { cwd: Some("~".into()), ..Default::default() });
            self.current = 0;
        }
    }

    /// Every pane the window's workspaces, pop-outs and roaming terminal
    /// show.
    fn shown_ids(&self) -> Vec<PaneId> {
        let mut ids = Vec::new();
        for ws in &self.workspaces {
            ws.root.panes(&mut ids);
        }
        ids.extend(self.popouts.iter().map(|p| p.pane));
        ids.extend(self.roam);
        ids
    }

    /// A shared layout's panes, for machine `k`: each terminal shown by a
    /// pane here that `used` does not have already, else a new view of it
    /// from what the session gave; one this window can not show yet (shown
    /// here already, and not given again) added to `missing`, and left out
    /// until it comes; one gone (an ended shell, empty space), left out.
    fn node_for(&mut self, spec: &Spec, k: usize, used: &mut Vec<PaneId>, missing: &mut Vec<u64>) -> Option<Node> {
        match spec {
            Spec::Pane { id: Some(sid), host: None, .. } => {
                let base = app_id(k, *sid);
                if let Some(v) = self.views(base).into_iter().find(|v| !used.contains(v)) {
                    used.push(v);
                    return Some(Node::Pane(v));
                }
                if self.held.contains_key(&base) {
                    self.target = k;
                    let v = self.spawn(Start { cmd: String::new(), cwd: pty::home(), pane: Some(*sid), host: None });
                    used.push(v);
                    return Some(Node::Pane(v));
                }
                if !self.views(base).is_empty() {
                    missing.push(*sid);
                }
                None
            }
            Spec::Across { v: kids, sizes } | Spec::Down { h: kids, sizes } => {
                let dir = if matches!(spec, Spec::Across { .. }) { Dir::Across } else { Dir::Down };
                let mut children = Vec::new();
                let mut shares = Vec::new();
                for (i, kid) in kids.iter().enumerate() {
                    if let Some(n) = self.node_for(kid, k, used, missing) {
                        children.push(n);
                        shares.push(sizes.get(i).copied().filter(|s| *s > 0.0).unwrap_or(1.0 / kids.len().max(1) as f32));
                    }
                }
                match children.len() {
                    0 => None,
                    1 => children.pop(),
                    _ => {
                        let total: f32 = shares.iter().sum();
                        Some(Node::Split { dir, children, sizes: shares.iter().map(|s| s / total).collect() })
                    }
                }
            }
            _ => None,
        }
    }

    /// Machine `k`'s workspaces put in the order of `ids` (the session's),
    /// in the places they have among the window's; the others where they
    /// are.
    fn order_like(&mut self, k: usize, ids: &[u64]) {
        let slots: Vec<usize> = (0..self.workspaces.len()).filter(|&wi| self.ws_machine(wi) == k && ids.contains(&self.workspaces[wi].id)).collect();
        if slots.len() < 2 {
            return;
        }
        let showing = self.workspaces.get(self.current).map(|w| w.id);
        let mut those: Vec<Workspace> = Vec::new();
        for &wi in slots.iter().rev() {
            those.push(self.workspaces.remove(wi));
        }
        those.sort_by_key(|w| ids.iter().position(|i| *i == w.id).unwrap_or(usize::MAX));
        for (&wi, w) in slots.iter().zip(those) {
            self.workspaces.insert(wi, w);
        }
        if let Some(wi) = showing.and_then(|id| self.workspaces.iter().position(|w| w.id == id)) {
            self.current = wi;
        }
    }
}
