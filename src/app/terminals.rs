//! Terminals apart from panes, as screen has windows apart from regions:
//! a terminal is a shell in a machine's session, running until it is
//! exited or killed; a pane is a place in a window that shows one. Closing
//! a pane leaves its terminal running. Any pane can be given any terminal:
//! from the sidebar (a click shows it in the pane you are in, Ctrl click
//! splits beside it, a drag puts it where it is dropped), from the picker
//! (Alt '), or by stepping the pane through the terminals no pane shows
//! (Alt , and Alt .).

use super::*;

/// Where a terminal taken from the session goes when it comes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Place {
    /// A workspace of its own (a whole workspace taken up).
    Workspace,
    /// Beside the pane you are in.
    Beside,
    /// Into this pane, in place of what it showed.
    Into(PaneId),
    /// Beside this pane, before it or after, across or down.
    Split(PaneId, Dir, bool),
}

/// A terminal no pane in this window shows: its machine, its name in that
/// machine's session, what to call it, whether it runs something now, and
/// whether another window shows it.
#[derive(Clone, Debug)]
pub(super) struct Free {
    pub k: usize,
    pub sid: u64,
    pub label: String,
    pub running: bool,
    pub elsewhere: bool,
}

/// A pane in a saved layout, found by its id: what it ran, or its folder's
/// name.
fn label_in(spec: &persist::Spec, id: u64) -> Option<String> {
    match spec {
        persist::Spec::Pane { cmd, cwd, id: Some(p), .. } if *p == id => {
            let cmd = cmd.trim();
            if !cmd.is_empty() {
                return Some(cmd.split_whitespace().next().unwrap_or(cmd).to_string());
            }
            let cwd = cwd.clone().unwrap_or_default();
            let name = cwd.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
            Some(if name.is_empty() || name == "~" { "shell".into() } else { name })
        }
        persist::Spec::Across { v, .. } => v.iter().find_map(|s| label_in(s, id)),
        persist::Spec::Down { h, .. } => h.iter().find_map(|s| label_in(s, id)),
        _ => None,
    }
}

impl App {
    /// The terminals no pane here shows, machine by machine, as their
    /// sessions last said: each on its own, whether it was left alone or in
    /// a workspace no window shows.
    pub(super) fn free_terminals(&self) -> Vec<Free> {
        let mut out = Vec::new();
        for (k, m) in self.machines.iter().enumerate() {
            for l in &m.listing {
                if !l.label.is_empty() {
                    if let Some(&sid) = l.panes.first() {
                        out.push(Free { k, sid, label: l.label.clone(), running: l.running, elsewhere: l.elsewhere });
                    }
                    continue;
                }
                let spec = toml::from_str::<WorkspaceSpec>(&l.spec).ok();
                for &sid in &l.panes {
                    let label = spec.as_ref().and_then(|s| s.layout.as_ref()).and_then(|lay| label_in(lay, sid)).unwrap_or_else(|| "shell".into());
                    out.push(Free { k, sid, label, running: false, elsewhere: l.elsewhere });
                }
            }
        }
        out
    }

    /// A terminal of machine `k`'s session brought into this window, `place`
    /// saying where; it comes when the session gives it.
    pub(super) fn take_terminal(&mut self, k: usize, sid: u64, place: Place) {
        if !self.machines.get(k).is_some_and(|m| m.client.connected()) {
            return;
        }
        let layout = persist::Spec::Pane { cmd: String::new(), cwd: None, pins: Vec::new(), empty: false, id: Some(sid), host: None };
        self.taking.push((k, WorkspaceSpec { layout: Some(layout), ..Default::default() }, place));
        self.machines[k].client.take(vec![sid]);
        // Off the list at once, so that stepping again goes on to the next;
        // the session's own list follows.
        for l in &mut self.machines[k].listing {
            l.panes.retain(|p| *p != sid);
        }
        self.machines[k].listing.retain(|l| !l.panes.is_empty());
        self.listed_at = Instant::now() - Duration::from_secs(60);
    }

    /// A pane no longer shown here, its terminal left running: what a pane
    /// holds of it let go (a search in it, commands waiting on it).
    pub(super) fn let_go(&mut self, id: PaneId) {
        self.forget_minimized(id);
        self.release_after(id, false);
        if self.find.as_ref().is_some_and(|f| f.pane == id) {
            self.find = None;
        }
        self.panes.remove(&id);
    }

    /// The pane you are in given the next terminal (`by` 1) or the one
    /// before (-1) of those no pane shows, as screen's Ctrl a n and p; the
    /// one it showed runs on, and is next in turn.
    pub(super) fn step_terminal(&mut self, by: i32) {
        let Some(at) = self.focused() else { return };
        if self.roam == Some(at) {
            return;
        }
        let free: Vec<Free> = self.free_terminals().into_iter().filter(|f| !f.elsewhere).collect();
        if free.is_empty() {
            return self.toast("No other terminals", "Every terminal is shown in a pane", None, None);
        }
        let i = if by > 0 { 0 } else { free.len() - 1 };
        let f = free[i].clone();
        self.take_terminal(f.k, f.sid, Place::Into(at));
    }

    /// The picker (Alt '): the terminals no pane here shows, to show one in
    /// the pane you are in.
    pub(super) fn terminal_picker(&mut self) {
        use crate::palette::{Action, Item};
        let free = self.free_terminals();
        if free.is_empty() {
            return self.toast("No other terminals", "Every terminal is shown in a pane", None, None);
        }
        let items = free
            .into_iter()
            .map(|f| {
                let host = (f.k > 0).then(|| self.machines.get(f.k).and_then(|m| m.host.clone())).flatten();
                let hint = match (host, f.elsewhere, f.running) {
                    (Some(h), _, _) => h,
                    (None, true, _) => "in another window".into(),
                    (None, false, true) => "running".into(),
                    _ => String::new(),
                };
                Some(Item { label: f.label, hint, action: Action::ShowTerminal(f.k, f.sid) })
            })
            .collect();
        self.open_items_menu(items);
    }

    /// Kill terminal: the one in the pane you are in ended, its pane with
    /// it; asked first when it runs a command.
    pub(super) fn kill_terminal(&mut self, sure: bool, event_loop: &ActiveEventLoop) {
        let Some(id) = self.focused() else { return };
        let running = self.panes.get(&id).and_then(|p| p.shell.running.as_ref().filter(|_| !p.shell.at_prompt).map(|r| r.2.trim().to_string()));
        if let (Some(cmd), false) = (running, sure) {
            use crate::palette::{Action, Item};
            let what = if cmd.is_empty() { "a command".to_string() } else { cmd };
            return self.open_items_menu(vec![Some(Item { label: format!("Kill it, and {what}"), hint: "it is running".into(), action: Action::KillTerminalNow })]);
        }
        if let Some(p) = self.panes.get_mut(&id) {
            p.pty.end = true;
        }
        self.close_pane(event_loop);
    }

    /// A terminal no pane here shows, ended.
    pub(super) fn kill_free(&mut self, k: usize, sid: u64) {
        if let Some(m) = self.machines.get(k) {
            m.client.close(sid);
        }
        self.listed_at = Instant::now() - Duration::from_secs(60);
    }
}
