//! Empty space: a pane made into nothing but room, to shape a layout a
//! window manager will not (a quarter of the screen in a half-screen
//! window). It keeps its place, so the lines beside it still drag; it is
//! drawn as plain ground, takes no focus, and moving between panes passes
//! it by. Its shell is ended. Right click it to make it a terminal again,
//! or to take it out.

use super::*;

impl App {
    /// Whether a pane is empty space, in any workspace.
    pub(super) fn is_space(&self, id: PaneId) -> bool {
        self.workspaces.iter().any(|w| w.spaces.contains(&id))
    }

    /// A pane made into empty space. The last terminal showing stays one.
    pub(super) fn make_space(&mut self, id: PaneId) {
        let Some(wi) = self.workspaces.iter().position(|w| w.root.contains(id)) else { return };
        let ws = &self.workspaces[wi];
        let mut ids = Vec::new();
        ws.root.panes(&mut ids);
        let terminals: Vec<PaneId> = ids.iter().copied().filter(|p| *p != id && !ws.spaces.contains(p) && !ws.minimized.contains(p)).collect();
        if terminals.is_empty() {
            return self.toast("One terminal left", "The last terminal showing stays one", None, Some(id));
        }
        // Its shell and what it ran, ended.
        if let Some(p) = self.panes.get(&id) {
            p.pty.signal(libc::SIGHUP, false);
        }
        let ws = &mut self.workspaces[wi];
        ws.spaces.push(id);
        ws.minimized.retain(|p| *p != id);
        ws.zoom = None;
        if ws.focus == id {
            ws.focus = terminals[0];
        }
        self.changed();
    }

    /// Empty space a terminal again: a fresh shell in its place, in the
    /// folder of the pane typed into.
    pub(super) fn space_to_terminal(&mut self, id: PaneId) {
        let Some(wi) = self.workspaces.iter().position(|w| w.spaces.contains(&id)) else { return };
        let cwd = self.focused().and_then(|f| self.panes.get(&f)).and_then(|p| p.pty.cwd()).unwrap_or_else(pty::home);
        self.target = crate::mux::split_id(id).0;
        let new = self.spawn(Start { cmd: String::new(), cwd, pane: None });
        let ws = &mut self.workspaces[wi];
        if ws.root.split_beside(id, new, Dir::Across, false) {
            ws.root.remove(id);
        }
        ws.spaces.retain(|p| *p != id);
        ws.focus = new;
        self.panes.remove(&id);
        self.changed();
    }

    /// Empty space taken out: the panes beside it take its room.
    pub(super) fn remove_space(&mut self, id: PaneId) {
        let Some(wi) = self.workspaces.iter().position(|w| w.spaces.contains(&id)) else { return };
        let ws = &mut self.workspaces[wi];
        ws.root.remove(id);
        ws.spaces.retain(|p| *p != id);
        self.panes.remove(&id);
        self.changed();
    }

    /// The empty space at a point of the window, in this workspace.
    pub(super) fn space_at(&self, x: i32, y: i32) -> Option<PaneId> {
        if self.roam_shown || self.ws().zoom.is_some() {
            return None;
        }
        self.ws().spaces.iter().copied().find(|id| self.panes.get(id).is_some_and(|p| p.rect.contains(x, y)))
    }

    /// A right click on empty space: make it a terminal, or take it out.
    pub(super) fn space_menu(&mut self) -> bool {
        let (x, y) = self.pointer;
        let Some(id) = self.space_at(x, y) else { return false };
        use crate::palette::{Action, Item};
        let c = |label: &str, action| Some(Item { label: label.into(), hint: String::new(), action });
        self.open_items_menu(vec![c("Make terminal", Action::SpaceToTerminal(id)), c("Remove space", Action::SpaceRemove(id))]);
        true
    }
}
