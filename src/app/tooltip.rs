//! Tooltips, as jog draws them: what a button does, and its key, in a
//! small box of its own with a little shadow, standing clear of the button
//! (under it, the buttons being along the tops of panes), kept inside the
//! window. Shown as soon as the pointer is on the button, gone as it leaves.

use super::*;

impl App {
    /// What the pointer is on that has a tooltip: the button's place, and
    /// what to say.
    pub(super) fn tooltip(&self) -> Option<(Rect, String)> {
        if self.menu.is_some() || self.palette.is_some() || self.settings.is_some() || self.drag.is_some() {
            return None;
        }
        let (x, y) = self.pointer;
        // A pane's caption: its buttons and its tags.
        if let Some((r, id, kind)) = self.multi_tags.iter().find(|(r, _, _)| r.contains(x, y)) {
            let zoomed = self.ws().zoom.is_some();
            let tee = self.panes.get(id).and_then(|p| p.tee.as_ref().map(|t| t.path.display().to_string())).unwrap_or_default();
            let words = match kind {
                watch::Tag::Close => "Close pane  ·  Alt X  ·  it runs on".to_string(),
                watch::Tag::Focus if zoomed => "Show all panes  ·  Alt Shift F".to_string(),
                watch::Tag::Focus => "This pane alone  ·  Alt Shift F".to_string(),
                watch::Tag::Minimize => "Minimize to a row  ·  it runs on".to_string(),
                watch::Tag::Restart => "Run the command again  ·  Ctrl R".to_string(),
                watch::Tag::Multi => "Typed into with the others  ·  click to leave it out".to_string(),
                watch::Tag::Tee => format!("Writing to {tee}  ·  click for more"),
                watch::Tag::TeeStop => "Stop writing to the file".to_string(),
                watch::Tag::Filter => "Only the lines that match  ·  click to stop".to_string(),
                watch::Tag::Notify => "Told when it ends  ·  click to stop".to_string(),
            };
            return Some((*r, words));
        }
        // The project bar's machine, and its workspaces' dots.
        if let Some(r) = self.foot_button.filter(|r| r.contains(x, y)) {
            return Some((r, "Another machine's workspaces".to_string()));
        }
        if let Some((r, d)) = self.foot_dots.iter().find(|(r, _)| r.contains(x, y)) {
            return Some((*r, self.foot_dot_words(*d)));
        }
        // The quake window's bar.
        if let Some((r, hit)) = self.quake_hits.iter().find(|(r, _)| r.contains(x, y)) {
            let words = match hit {
                quake::QuakeHit::Sidebar => if self.sidebar_open { "Hide the sidebar" } else { "Show the sidebar" }.to_string(),
                quake::QuakeHit::Workspaces => "Workspaces".to_string(),
                quake::QuakeHit::Pane(id) => self.terminal_label(*id),
                quake::QuakeHit::Grip => "Drag to make it taller or shorter".to_string(),
                quake::QuakeHit::Hide => "Hide  ·  jot --quake".to_string(),
            };
            return Some((*r, words));
        }
        // The sidebar's buttons (its rows say what they are).
        if x < self.sidebar_w() || self.side_toggle.is_some_and(|r| r.contains(x, y)) {
            if let Some(r) = self.side_toggle.filter(|r| r.contains(x, y)) {
                return Some((r, "Show the sidebar".to_string()));
            }
            let hit = self.side_hits.iter().filter(|(r, _)| r.contains(x, y)).min_by_key(|(r, _)| r.w);
            if let Some((r, hit)) = hit {
                let words = match hit {
                    sidebar::SideHit::Toggle => "Hide the sidebar".to_string(),
                    sidebar::SideHit::AddMachine => "Add a machine".to_string(),
                    sidebar::SideHit::NewTerminal(k) => {
                        let name = self.machines.get(*k).and_then(|m| m.host.clone()).unwrap_or_else(|| "this machine".into());
                        format!("New terminal on {name}")
                    }
                    _ => return None,
                };
                return Some((*r, words));
            }
        }
        None
    }

    /// The tooltip, over everything else.
    pub(super) fn draw_tooltip(&mut self) {
        let Some((anchor, words)) = self.tip_shown.clone() else { return };
        let c = self.cur_theme.chrome();
        let pad = self.px(7.0);
        let small = self.fonts.small.height;
        let (w, h) = (self.fonts.small.width(&words) + pad * 2, small + pad);
        let margin = self.px(4.0);
        let x = (anchor.x + anchor.w / 2 - w / 2).clamp(margin, (self.canvas.width - w - margin).max(margin));
        // Under the button; over it when there is no room below.
        let below = anchor.y + anchor.h + self.px(6.0);
        let y = if below + h + margin > self.canvas.height { anchor.y - h - self.px(6.0) } else { below };
        let r = Rect { x, y, w, h };
        self.canvas.panel(r, self.px(6.0), c.panel, c.edge);
        self.canvas.text(&mut self.fonts.small, &words, (x + pad) as f32, y + (h - small) / 2, c.text, r);
        let s = self.px(16.0);
        self.damage.push(Rect { x: r.x - s, y: r.y - s, w: r.w + 2 * s, h: r.h + 2 * s });
    }
}
