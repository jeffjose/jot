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
    /// Beside the pane you are in.
    Beside,
    /// Into this pane, in place of what it showed, which is kept under it
    /// (closing this one shows it again).
    Into(PaneId),
    /// Into this pane, the one under it come back (the one shown closed).
    Back(PaneId),
    /// Beside this pane, before it or after, across or down.
    Split(PaneId, Dir, bool),
    /// A workspace of its own, laid out as it was where it was held.
    Workspace,
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
        persist::Spec::Pane { cmd, cwd, id: Some(p), .. } if *p == id => Some(term_name(cwd.as_deref().unwrap_or(""), Some(cmd))),
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
        // A workspace another window shows is listed whole: those of its
        // terminals a pane here shows too are not free.
        let here: std::collections::HashSet<(usize, u64)> = self.panes.keys().map(|id| crate::mux::split_id(*id)).collect();
        for (k, m) in self.machines.iter().enumerate() {
            for l in &m.listing {
                if !l.label.is_empty() {
                    if let Some(&sid) = l.panes.first().filter(|s| !here.contains(&(k, **s))) {
                        out.push(Free { k, sid, label: term_name(&l.label, None), running: l.running, elsewhere: l.elsewhere });
                    }
                    continue;
                }
                let spec = toml::from_str::<WorkspaceSpec>(&l.spec).ok();
                for &sid in l.panes.iter().filter(|s| !here.contains(&(k, **s))) {
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
        // Its row as it is now, to stay so until a pane shows it.
        if !self.term_seen.contains_key(&(k, sid)) {
            if let Some(t) = self.side_terminals().into_iter().find(|t| (t.k, t.sid) == (k, sid)) {
                self.term_seen.insert((k, sid), (t.label, t.cmd, t.status));
            }
        }
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
        // Down the sidebar's list, every terminal in it, round the end: from
        // the one this pane shows (or is about to, a step still on its way)
        // to the next (Alt .) or the one before (Alt ,), whatever shows it: it
        // comes into this pane, as a second view when another pane shows it
        // (here or in another workspace, which keep it), so the pane always
        // shows n+1, or n-1. Each terminal once, in the order of its number.
        let at = self.replaced_by(at);
        let here = self.stepping.filter(|(p, _)| *p == at).map(|(_, t)| t).unwrap_or_else(|| crate::mux::split_id(at));
        let mut ring: Vec<(usize, u64)> = Vec::new();
        for t in self.side_terminals() {
            if !ring.contains(&(t.k, t.sid)) {
                ring.push((t.k, t.sid));
            }
        }
        if !ring.contains(&here) {
            ring.push(here);
        }
        let numbers = &self.term_numbers;
        ring.sort_by_key(|t| (t.0, numbers.get(t).copied().unwrap_or(usize::MAX), t.1));
        if ring.len() < 2 {
            return self.toast("No other terminals", "This is the only one", None, None);
        }
        let i = ring.iter().position(|t| *t == here).unwrap_or(0) as i32;
        let n = ring.len() as i32;
        let (k, sid) = ring[(i + by).rem_euclid(n) as usize];
        self.stepping = Some((at, (k, sid)));
        self.take_terminal(k, sid, Place::Into(at));
    }

    /// The pane that stands where `id` stood, a step having put another
    /// terminal there.
    pub(super) fn replaced_by(&self, mut id: PaneId) -> PaneId {
        // A pane's id is its terminal's, so ids come round again: a pane
        // that is there is the answer, and the trail is followed only so far.
        for _ in 0..32 {
            if self.workspaces.iter().any(|w| w.root.contains(id)) {
                return id;
            }
            match self.replaced.iter().rev().find(|(old, _)| *old == id) {
                Some((_, new)) => id = *new,
                None => return id,
            }
        }
        id
    }

    /// A terminal a pane here stopped showing, listed as running on at once,
    /// before its session's own list says so.
    pub(super) fn list_free(&mut self, id: PaneId) {
        let (k, sid) = crate::mux::split_id(id);
        let Some(p) = self.panes.get(&id) else { return };
        let running = pane_running(p);
        let label = running.clone().unwrap_or_else(|| p.pty.cwd().map(|c| persist::shorten(&c)).unwrap_or_else(|| "~".into()));
        if let Some(m) = self.machines.get_mut(k) {
            if !m.listing.iter().any(|l| l.panes.contains(&sid)) {
                m.listing.push(crate::mux::Listed { spec: String::new(), panes: vec![sid], elsewhere: false, label, running: running.is_some() });
                self.let_go_at.push((k, sid, Instant::now()));
            }
        }
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
                    (None, _, true) => "running".into(),
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

impl App {
    /// A new terminal on machine `k`, in a pane beside the one you are in:
    /// in that pane's folder here, in its home on another machine.
    pub(super) fn new_terminal_on(&mut self, k: usize) {
        if self.roam_shown || self.workspaces.is_empty() {
            return;
        }
        let at = self.ws().focus;
        let cwd = if k > 0 { PathBuf::from("~") } else { self.panes.get(&at).and_then(|p| p.pty.cwd()).unwrap_or_else(pty::home) };
        self.target = k;
        let id = self.spawn(Start { cmd: String::new(), cwd, pane: None, host: None });
        let ws = self.ws_mut();
        ws.zoom = None;
        ws.root.split_beside(at, id, Dir::Across, false);
        ws.focus = id;
        self.changed();
    }
}

/// A terminal's name, the same whether a pane shows it or not: the program
/// it runs (its first word, without its folder), else the last part of the
/// folder it is in ("~" at home).
pub(super) fn term_name(place: &str, running: Option<&str>) -> String {
    if let Some(cmd) = running.map(str::trim).filter(|c| !c.is_empty()) {
        let first = cmd.split_whitespace().next().unwrap_or(cmd);
        return first.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or(first).to_string();
    }
    let p = place.trim().trim_end_matches('/');
    if p.is_empty() || p == "~" {
        return "~".into();
    }
    // Not a folder (what it ran, as a session says it): its program.
    if !p.starts_with('/') && !p.starts_with('~') {
        return term_name("", Some(p));
    }
    p.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("/").to_string()
}

impl App {
    /// The name of a terminal a pane here shows, as term_name gives it.
    pub(super) fn pane_term_name(&self, p: &Pane) -> String {
        let running = pane_running(p);
        let place = p.pty.cwd().map(|c| persist::shorten(&c)).unwrap_or_else(|| p.caption.clone());
        term_name(&place, running.as_deref())
    }
}

impl App {
    /// The name given to a terminal, if one was: read from the file every
    /// window shares, so a name given in one holds in all of them.
    pub(super) fn given_name(&self, k: usize, sid: u64) -> Option<String> {
        let host = self.machines.get(k).and_then(|m| m.host.clone()).unwrap_or_default();
        self.term_names.iter().find(|(h, s, _)| *h == host && *s == sid).map(|(_, _, n)| n.clone())
    }

    /// A terminal given a name; an empty one takes its name back.
    pub(super) fn name_terminal(&mut self, k: usize, sid: u64, name: &str) {
        let host = self.machines.get(k).and_then(|m| m.host.clone()).unwrap_or_default();
        self.term_names = persist::load_term_names();
        self.term_names.retain(|(h, s, _)| !(*h == host && *s == sid));
        let name = name.trim();
        if !name.is_empty() {
            self.term_names.push((host, sid, name.to_string()));
        }
        persist::save_term_names(&self.term_names);
        self.full = true;
        self.redraw();
    }

    /// The box to rename a terminal, its name now all selected.
    pub(super) fn rename_terminal_prompt(&mut self, k: usize, sid: u64) {
        let now = self.given_name(k, sid).or_else(|| self.side_terminals().into_iter().find(|t| t.k == k && t.sid == sid).map(|t| t.label)).unwrap_or_default();
        let mut field = crate::field::TextField::new(&now);
        field.anchor = Some(0);
        self.palette = Some(crate::palette::Palette { mode: crate::palette::Mode::RenameTerminal(k, sid), field, matches: Vec::new(), selected: 0, first: 0 });
        self.full = true;
        self.redraw();
    }
}

impl App {
    /// What a pane's terminal is called: the name given it, else
    /// term_name's.
    pub(super) fn terminal_label(&self, id: PaneId) -> String {
        let (k, sid) = crate::mux::split_id(id);
        self.given_name(k, sid).or_else(|| self.panes.get(&id).map(|p| self.pane_term_name(p))).unwrap_or_default()
    }
}

impl App {
    /// A machine taken out of the sidebar: no longer listed once it holds
    /// nothing of this window's; its terminals run on there.
    pub(super) fn remove_machine(&mut self, k: usize) {
        if k == 0 {
            return;
        }
        if let Some(m) = self.machines.get_mut(k) {
            m.added = false;
        }
        persist::save_machines(&self.machines.iter().skip(1).filter(|m| m.added).filter_map(|m| m.host.clone()).collect::<Vec<_>>());
        self.full = true;
        self.redraw();
    }
}

impl App {
    /// The panes here showing the terminal `id` is (or is a view of): the
    /// one that answers the program first.
    pub(super) fn views(&self, id: PaneId) -> Vec<PaneId> {
        let base = crate::mux::base_id(id);
        let mut v: Vec<PaneId> = self.panes.keys().copied().filter(|p| crate::mux::base_id(*p) == base).collect();
        v.sort();
        if let Some(first) = self.panes.get(&id).or_else(|| v.first().and_then(|p| self.panes.get(p))).and_then(|p| p.pty.views.borrow().first().copied()) {
            if let Some(i) = v.iter().position(|p| *p == first) {
                v.swap(0, i);
            }
        }
        v
    }
}

impl App {
    /// Each terminal's row as shown noted, and numbers for terminals new
    /// since last time (the lowest free, in the
    /// order their sessions made them); those of terminals gone, free again.
    pub(super) fn number_terminals(&mut self) {
        let rows = self.side_terminals();
        // Rows panes here show, as they show them: kept for when none does.
        for t in rows.iter().filter(|t| t.pane.is_some()) {
            self.term_seen.insert((t.k, t.sid), (t.label.clone(), t.cmd.clone(), t.status));
        }
        let mut live: Vec<(usize, u64)> = rows.into_iter().map(|t| (t.k, t.sid)).collect();
        // On its way into a pane, in no list for now: still its own.
        for (k, spec, _) in &self.taking {
            live.extend(spec.layout.as_ref().map(persist::ids_in_order).unwrap_or_default().into_iter().map(|sid| (*k, sid)));
        }
        let before = self.term_numbers.len();
        self.term_numbers.retain(|t, _| live.contains(t));
        self.term_seen.retain(|t, _| live.contains(t));
        let mut changed = self.term_numbers.len() != before;
        for t in live {
            if !self.term_numbers.contains_key(&t) {
                let n = (1..).find(|n| !self.term_numbers.values().any(|v| v == n)).unwrap_or(1);
                self.term_numbers.insert(t, n);
                changed = true;
            }
        }
        if changed {
            self.full = true;
        }
    }

    /// The number of the terminal pane `id` shows.
    pub(super) fn term_number(&self, id: PaneId) -> Option<usize> {
        self.term_numbers.get(&crate::mux::split_id(id)).copied()
    }
}

/// What a pane's terminal runs now: the command as its shell marked it;
/// with no mark (a shell that makes none, or one lost while a program
/// redraws), the program in front, as the session's process table has it,
/// the same the session names one no pane shows by. None at a prompt.
pub(super) fn pane_running(p: &Pane) -> Option<String> {
    let marked = p.shell.running.as_ref().filter(|_| !p.shell.at_prompt).map(|r| r.2.trim().to_string()).filter(|s| !s.is_empty());
    marked.or_else(|| p.pty.busy().then(|| p.pty.foreground()).flatten())
}

impl App {
    /// Terminal `sid` of machine `k` shown in a new jot window as well, as a
    /// link opened in a new one: the window starts taking it up (--show),
    /// on that machine's session (jot ssh, for another machine's).
    pub(super) fn open_in_new_window(&mut self, k: usize, sid: u64) {
        let Some(m) = self.machines.get(k) else { return };
        let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("jot"));
        let sid = sid.to_string();
        let mut args: Vec<&std::ffi::OsStr> = Vec::new();
        let host = m.client.host.clone();
        if let Some(h) = &host {
            args.push("ssh".as_ref());
            args.push(h.as_ref());
        }
        args.push("--show".as_ref());
        args.push(sid.as_ref());
        super::open_detached(&exe, &args);
    }
}
