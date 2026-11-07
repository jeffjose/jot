//! A pane's own command line, on Alt ;: a slim box at the bottom of the
//! pane, pushing its text up, that works whatever the pane is running.
//! A command typed in it runs at the prompt when the shell is waiting,
//! or waits its turn behind the command running; each in the queue can
//! be taken out (×) or run now (▶), beside the shell rather than in it,
//! in the pane's folder, with $JOB the running job's process group. A
//! `/` opens the slash commands, these among them: /stop and /kill, for
//! the job running.

use super::slash::Row;
use super::*;
use crate::field::TextField;

/// The command line open, for a pane.
pub struct Bar {
    pub pane: PaneId,
    pub field: TextField,
    /// Where in the history ↑ ↓ have got to, None for what is being typed.
    pub back: Option<usize>,
    /// The slash command picked, when a `/` is typed.
    pub selected: usize,
}

/// What can be clicked on a bar.
#[derive(Clone, Copy, Debug)]
pub enum BarHit {
    /// Opens the bar, from the strip a closed one leaves.
    Open,
    Remove(usize),
    /// A command waiting on another pane, taken out.
    RemoveAfter(usize),
    RunNow(usize),
    /// Lets a queue held by a failed command go on.
    Resume,
    /// Clears what a command run now printed.
    Dismiss,
}

const QUEUE_SHOWN: usize = 5;
const OUTPUT_SHOWN: usize = 10;

impl App {
    fn bar_row_h(&self) -> i32 {
        self.fonts.ui.height + self.px(12.0)
    }

    /// The rows the bar takes at the bottom of a pane: its own rows (the
    /// strip a waiting queue leaves, or the open bar's), and the lines of
    /// output it shows, which are set closer, as a terminal's.
    fn bar_rows(&self, id: PaneId) -> (usize, usize) {
        let Some(p) = self.panes.get(&id) else { return (0, 0) };
        let s = &p.shell;
        if self.bar.as_ref().is_some_and(|b| b.pane == id) {
            let (head, out) = s.ran.as_ref().map_or((0, 0), |(_, text, _)| (1, text.lines().count().clamp(1, OUTPUT_SHOWN)));
            (1 + s.queue.len().min(QUEUE_SHOWN) + s.after.len().min(QUEUE_SHOWN) + head, out)
        } else {
            (usize::from(!s.queue.is_empty() || !s.after.is_empty()), 0)
        }
    }

    /// The height of a line of output in the bar.
    fn bar_line_h(&self) -> i32 {
        self.fonts.mono.height + self.px(4.0)
    }

    /// The height the bar takes from a pane.
    pub(super) fn bar_height(&self, id: PaneId) -> i32 {
        match self.bar_rows(id) {
            (0, 0) => 0,
            (rows, out) => rows as i32 * self.bar_row_h() + out as i32 * self.bar_line_h() + self.px(if out > 0 { 12.0 } else { 6.0 }),
        }
    }

    /// Alt ;: the focused pane's command line, open; open already, shut.
    pub(super) fn toggle_bar(&mut self) {
        if let Some(b) = self.bar.take() {
            self.forget_ran(b.pane);
        } else {
            let Some(id) = self.focused() else { return };
            self.close_slash();
            self.bar = Some(Bar { pane: id, field: TextField::default(), back: None, selected: 0 });
        }
        self.relayout();
    }

    /// The bar shut: what a finished run printed goes with it.
    fn forget_ran(&mut self, id: PaneId) {
        if let Some(p) = self.panes.get_mut(&id) {
            if p.shell.ran.as_ref().is_some_and(|r| r.2.is_some()) {
                p.shell.ran = None;
            }
        }
    }

    pub(super) fn close_bar(&mut self) {
        if let Some(b) = self.bar.take() {
            self.forget_ran(b.pane);
            self.relayout();
        }
    }

    /// The slash commands for what is typed, when it starts with `/`.
    fn bar_rows_for(&self, text: &str) -> Vec<Row> {
        match text.strip_prefix('/') {
            Some(typed) => self.slash_rows(typed, false),
            None => Vec::new(),
        }
    }

    /// A key while the bar is open: all of them are its.
    pub(super) fn bar_key(&mut self, event: &winit::event::KeyEvent, event_loop: &ActiveEventLoop) {
        let PhysicalKey::Code(code) = event.physical_key else { return };
        let Some(b) = &self.bar else { return };
        // The keyboard went to another pane: the bar goes.
        if Some(b.pane) != self.focused() {
            return self.close_bar();
        }
        let text = b.field.string();
        let rows = self.bar_rows_for(&text);
        match code {
            KeyCode::Escape => return self.close_bar(),
            KeyCode::Enter | KeyCode::NumpadEnter => {
                if let Some(row) = rows.get(b.selected).cloned() {
                    return self.bar_slash(&row, event_loop);
                }
                return self.bar_submit();
            }
            KeyCode::Tab => {
                // The pick's name filled in, and a space after one that
                // takes a word.
                if let Some(row) = rows.get(b.selected) {
                    let name = row.label.strip_suffix(" …").map(|n| format!("/{n} ")).unwrap_or_else(|| format!("/{}", row.label));
                    if let Some(b) = &mut self.bar {
                        b.field = TextField::new(&name);
                        b.selected = 0;
                    }
                }
            }
            KeyCode::ArrowUp | KeyCode::ArrowDown if !rows.is_empty() => {
                if let Some(b) = &mut self.bar {
                    b.selected = if code == KeyCode::ArrowDown { (b.selected + 1).min(rows.len() - 1) } else { b.selected.saturating_sub(1) };
                }
            }
            KeyCode::ArrowUp | KeyCode::ArrowDown => {
                // Back through what was run from here, and forth again.
                let history = self.panes.get(&b.pane).map(|p| p.shell.history.clone()).unwrap_or_default();
                if let Some(b) = &mut self.bar {
                    let back = match (code, b.back) {
                        (KeyCode::ArrowUp, None) if !history.is_empty() => Some(history.len() - 1),
                        (KeyCode::ArrowUp, Some(i)) => Some(i.saturating_sub(1)),
                        (KeyCode::ArrowDown, Some(i)) if i + 1 < history.len() => Some(i + 1),
                        (KeyCode::ArrowDown, Some(_)) => None,
                        (_, at) => at,
                    };
                    b.back = back;
                    b.field = TextField::new(back.and_then(|i| history.get(i)).map_or("", String::as_str));
                }
            }
            // Ctrl C, as at a prompt: the line cleared; an empty one, the
            // bar shut. With text selected it copies, as it does elsewhere.
            KeyCode::KeyC if self.mods.control_key() && !self.mods.shift_key() && b.field.selection().is_none() => {
                if text.is_empty() {
                    return self.close_bar();
                }
                if let Some(b) = &mut self.bar {
                    b.field = TextField::default();
                    b.back = None;
                    b.selected = 0;
                }
            }
            // A space after !!, !str, !?str or !$: the word made what it
            // stands for, from what the pane ran, as tcsh's magic space.
            KeyCode::Space if !self.mods.control_key() && !self.mods.alt_key() && self.bar_expand() => {}
            _ => {
                let pasted = if self.mods.control_key() && code == KeyCode::KeyV { self.clip_get(false) } else { None };
                let copied: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
                let mods = self.mods;
                if let Some(b) = &mut self.bar {
                    let copied = &copied;
                    if b.field.key(code, mods, event.text.as_deref(), &mut || pasted.clone(), &mut |t| *copied.borrow_mut() = Some(t)) == crate::field::Outcome::Changed {
                        b.selected = 0;
                    }
                }
                if let Some(t) = copied.into_inner() {
                    self.clip_set(t, false);
                }
            }
        }
        self.bar_changed();
    }

    /// The bar drawn again, the pane laid out again if its height changed.
    fn bar_changed(&mut self) {
        if let Some(id) = self.bar.as_ref().map(|b| b.pane) {
            if let Some(p) = self.panes.get_mut(&id) {
                p.full = true;
            }
        }
        self.full = true;
        self.relayout();
    }

    /// Whether the pane's shell is waiting at its prompt, so a command can
    /// be typed into it now; without the shell's marks, it is taken to be.
    fn waiting(&self, id: PaneId) -> bool {
        self.panes.get(&id).is_some_and(|p| !p.shell.marked || (p.shell.at_prompt && !p.pty.busy()))
    }

    /// Enter: the command run at the prompt, or put in the queue behind
    /// what is running.
    pub(super) fn bar_submit(&mut self) {
        let Some(b) = &self.bar else { return };
        let (id, cmd) = (b.pane, b.field.string().trim().to_string());
        // A command ending in /edit: it, in the long-command editor, and
        // back here after.
        if let Some(before) = cmd.strip_suffix("/edit").filter(|b| b.is_empty() || b.ends_with(char::is_whitespace)) {
            let before = before.trim().to_string();
            if !before.is_empty() {
                return self.open_editor_with(id, &before, true);
            }
        }
        if cmd.is_empty() {
            return;
        }
        // @N cmd: waits for pane N's command to end, then runs here. If
        // pane N is not running anything, it runs as any other would.
        let (cmd, after) = match cmd.strip_prefix('@').and_then(|r| r.split_once(char::is_whitespace)) {
            Some((name, rest)) => match self.pane_by_number(name) {
                Some(t) if t != id && !self.waiting(t) => (rest.trim().to_string(), Some(t)),
                Some(_) => (rest.trim().to_string(), None),
                None => {
                    self.toast("No such pane", &format!("@{name}: pane N in this workspace, or W.P"), None, Some(id));
                    return;
                }
            },
            None => (cmd, None),
        };
        if let Some(t) = after {
            if let Some(p) = self.panes.get_mut(&id) {
                p.shell.after.push((t, cmd));
            }
            if let Some(b) = &mut self.bar {
                b.field = TextField::default();
                b.back = None;
            }
            return self.bar_changed();
        }
        let waiting = self.waiting(id);
        let Some(p) = self.panes.get_mut(&id) else { return };
        p.shell.history.retain(|h| *h != cmd);
        p.shell.history.push(cmd.clone());
        if waiting {
            // Whatever was half typed at the prompt goes first (Ctrl U).
            p.pty.write(format!("\x15{cmd}\r").as_bytes());
            p.shell.entered = Some(cmd);
            self.bar = None;
        } else {
            p.shell.queue.push(cmd);
            if let Some(b) = &mut self.bar {
                b.field = TextField::default();
                b.back = None;
            }
        }
        self.bar_changed();
    }

    /// A slash command taken in the bar: it applies to the bar's pane, and
    /// what would go on the command line goes in the bar instead.
    fn bar_slash(&mut self, row: &Row, event_loop: &ActiveEventLoop) {
        let Some(id) = self.bar.as_ref().map(|b| b.pane) else { return };
        // One that takes a word, with none yet: its name in the box.
        if let Some(name) = row.label.strip_suffix(" …") {
            if let Some(b) = &mut self.bar {
                b.field = TextField::new(&format!("/{name} "));
                b.selected = 0;
            }
            return self.bar_changed();
        }
        let label = row.label.clone();
        let (name, arg) = label.split_once(' ').map(|(n, a)| (n.to_string(), a.to_string())).unwrap_or((label.clone(), String::new()));
        let Some(c) = self.slash_commands().into_iter().find(|c| c.name == name) else { return };
        use super::slash::Does;
        let put = |app: &mut App, text: &str| {
            if let Some(b) = &mut app.bar {
                b.field = TextField::new(text);
                b.selected = 0;
            }
            app.bar_changed();
        };
        match c.does {
            Does::Insert(text) => put(self, &text),
            Does::Run(cmd) => {
                put(self, &cmd);
                self.bar_submit();
            }
            Does::Stop => {
                self.stop_job(id);
                self.close_bar();
            }
            Does::Edit => {
                self.close_bar();
                let text = self.prompt_line(id);
                self.open_editor_with(id, &text, false);
            }
            Does::Tree | Does::Ports | Does::Files => {
                let (title, lines) = match c.does {
                    Does::Tree => ("/tree", self.report_tree(id)),
                    Does::Ports => ("/ports", self.report_ports(id)),
                    _ => ("/files", self.report_files(id)),
                };
                self.bar_report(id, title, lines);
                if let Some(b) = &mut self.bar {
                    b.field = TextField::default();
                    b.selected = 0;
                }
                self.bar_changed();
            }
            Does::Notify => {
                self.set_notify(id, true);
                self.close_bar();
            }
            Does::Info => {
                self.bar_info(id);
                if let Some(b) = &mut self.bar {
                    b.field = TextField::default();
                    b.selected = 0;
                }
                self.bar_changed();
            }
            Does::Kill => {
                self.kill_job(id);
                self.close_bar();
            }
            other => {
                self.close_bar();
                self.slash_apply(id, other, &arg, event_loop);
            }
        }
    }

    /// The pane's running job's process group, when something other than
    /// the shell has the terminal.
    fn job(&self, id: PaneId) -> Option<libc::pid_t> {
        let p = self.panes.get(&id)?;
        let pgrp = unsafe { libc::tcgetpgrp(p.pty.fd) };
        (pgrp > 0 && pgrp != p.pty.pid).then_some(pgrp)
    }

    /// Ctrl C to the pane, as pressing it there does.
    pub(super) fn stop_job(&mut self, id: PaneId) {
        if let Some(p) = self.panes.get(&id) {
            p.pty.write(&[0x03]);
        }
    }

    /// The running job killed: SIGTERM, and SIGKILL if asked again within
    /// a few seconds.
    pub(super) fn kill_job(&mut self, id: PaneId) {
        let Some(pgrp) = self.job(id) else {
            return self.toast("Nothing running", "The shell has the pane to itself", None, Some(id));
        };
        let name = self.panes.get(&id).and_then(|p| p.pty.foreground()).unwrap_or_else(|| "the job".into());
        let Some(p) = self.panes.get_mut(&id) else { return };
        let hard = p.shell.killed_at.is_some_and(|t| t.elapsed() < Duration::from_secs(10));
        let (signal, what) = if hard { (libc::SIGKILL, "SIGKILL") } else { (libc::SIGTERM, "SIGTERM") };
        unsafe { libc::kill(-pgrp, signal) };
        p.shell.killed_at = Some(Instant::now());
        let again = if hard { "" } else { " · /kill again for SIGKILL" };
        self.toast(&format!("Sent {what}"), &format!("to {name}{again}"), None, Some(id));
    }

    /// A queued command run now, beside the shell rather than in it: in
    /// the pane's folder, with $JOB its running job's process group. What
    /// it prints is shown in the bar as it comes.
    fn run_now(&mut self, id: PaneId, i: usize) {
        let Some(p) = self.panes.get_mut(&id) else { return };
        if i >= p.shell.queue.len() {
            return;
        }
        let cmd = p.shell.queue.remove(i);
        let cwd = p.pty.cwd().unwrap_or_else(pty::home);
        p.shell.ran = Some((cmd.clone(), String::new(), None));
        let job = self.job(id).map(|g| g.to_string()).unwrap_or_default();
        let (done, proxy) = (self.ran.clone(), self.proxy.clone());
        std::thread::spawn(move || {
            let out = std::process::Command::new("sh").arg("-c").arg(format!("exec 2>&1; {cmd}")).current_dir(cwd).env("JOB", job).stdin(std::process::Stdio::null()).output();
            let (text, status) = match out {
                Ok(o) => (String::from_utf8_lossy(&o.stdout).into_owned(), o.status.code().unwrap_or(-1)),
                Err(e) => (e.to_string(), -1),
            };
            if let Ok(mut d) = done.lock() {
                d.push((id, text, status));
            }
            let _ = proxy.send_event(());
        });
        self.bar_changed();
    }

    /// What commands run now printed, to their bars.
    pub(super) fn collect_ran(&mut self) {
        let finished: Vec<(PaneId, String, i32)> = self.ran.lock().map(|mut d| std::mem::take(&mut *d)).unwrap_or_default();
        if finished.is_empty() {
            return;
        }
        for (id, text, status) in finished {
            if let Some(p) = self.panes.get_mut(&id) {
                if let Some(ran) = &mut p.shell.ran {
                    ran.1 = text;
                    ran.2 = Some(status);
                }
            }
        }
        self.bar_changed();
    }

    /// A click on a bar: true when it was on one.
    pub(super) fn bar_click(&mut self) -> bool {
        let (x, y) = self.pointer;
        let Some((_, id, hit)) = self.bar_hits.iter().find(|(r, _, _)| r.contains(x, y)).cloned() else { return false };
        match hit {
            BarHit::Open => {
                self.set_focus(id);
                if self.bar.as_ref().is_none_or(|b| b.pane != id) {
                    self.bar = None;
                    self.toggle_bar();
                }
            }
            BarHit::Remove(i) => {
                if let Some(p) = self.panes.get_mut(&id) {
                    if i < p.shell.queue.len() {
                        p.shell.queue.remove(i);
                    }
                    if p.shell.queue.is_empty() {
                        p.shell.paused = false;
                    }
                }
                self.bar_changed();
                self.relayout();
            }
            BarHit::RunNow(i) => self.run_now(id, i),
            BarHit::RemoveAfter(i) => {
                if let Some(p) = self.panes.get_mut(&id) {
                    if i < p.shell.after.len() {
                        p.shell.after.remove(i);
                    }
                }
                self.bar_changed();
            }
            BarHit::Resume => {
                if let Some(p) = self.panes.get_mut(&id) {
                    p.shell.paused = false;
                }
                // At the prompt already: the next one goes now.
                if self.waiting(id) {
                    self.run_next(id);
                }
                self.bar_changed();
            }
            BarHit::Dismiss => {
                if let Some(p) = self.panes.get_mut(&id) {
                    p.shell.ran = None;
                }
                self.bar_changed();
            }
        }
        true
    }

    /// The next queued command, typed at the prompt.
    pub(super) fn run_next(&mut self, id: PaneId) {
        let Some(p) = self.panes.get_mut(&id) else { return };
        if p.shell.paused || p.shell.queue.is_empty() || !p.shell.at_prompt {
            return;
        }
        let next = p.shell.queue.remove(0);
        p.pty.write(format!("\x15{next}\r").as_bytes());
        p.shell.entered = Some(next);
        self.full = true;
        self.relayout();
    }

    /// The bar of a pane, or the strip its waiting queue leaves, at the
    /// bottom of the pane above its caption.
    pub(super) fn draw_bar(&mut self, id: PaneId) {
        let h = self.bar_height(id);
        if h == 0 {
            return;
        }
        let Some(p) = self.panes.get(&id) else { return };
        let cap = if self.config.captions { self.caption_h() } else { 0 };
        let area = Rect { x: p.rect.x, y: p.rect.y + p.rect.h - cap - h, w: p.rect.w, h };
        let (queue, paused, ran, after) = (p.shell.queue.clone(), p.shell.paused, p.shell.ran.clone(), p.shell.after.clone());
        // Each pane waited on, named as @ names it.
        let after: Vec<(String, String)> = after.into_iter().map(|(t, cmd)| (self.pane_number(t), cmd)).collect();
        let busy = !self.waiting(id);
        let c = self.cur_theme.chrome();
        let accent = self.cur_accent;
        let scale = self.scale;
        let px = |v: f32| (v * scale).round() as i32;
        let (uh, sh) = (self.fonts.ui.height, self.fonts.small.height);
        let row_h = self.bar_row_h();
        let (mh, line_h) = (self.fonts.mono.height, self.bar_line_h());
        let open = self.bar.as_ref().is_some_and(|b| b.pane == id);
        self.bar_hits.retain(|(_, p, _)| *p != id);
        self.canvas.fill(area, c.side);
        self.canvas.fill(Rect { h: 1, ..area }, c.edge);
        let pad = px(12.0);
        let mut y = area.y + px(4.0);
        let ib = row_h - px(8.0);
        // A small square button at the right of a row, with its glyph.
        let button = |app: &mut App, x: i32, y: i32, hit: BarHit, play: bool| {
            let r = Rect { x, y: y + (row_h - ib) / 2, w: ib, h: ib };
            let lit = r.contains(app.pointer.0, app.pointer.1);
            if lit {
                app.canvas.round_fill(r, px(5.0), c.hover, 255);
            }
            let (cx, cy, arm, dot) = (r.x + ib / 2, r.y + ib / 2, px(4.0), px(2.0).max(2));
            if play {
                for t in 0..=arm * 2 {
                    let half = (arm * 2 - t) / 2;
                    app.canvas.fill(Rect { x: cx - arm + 1 + t, y: cy - half, w: 1, h: half * 2 + 1 }, if lit { accent } else { c.dim });
                }
            } else {
                for t in -arm..=arm {
                    app.canvas.round_fill(Rect { x: cx + t - dot / 2, y: cy + t - dot / 2, w: dot, h: dot }, dot / 2, if lit { 0xE5484D } else { c.dim }, 255);
                    app.canvas.round_fill(Rect { x: cx + t - dot / 2, y: cy - t - dot / 2, w: dot, h: dot }, dot / 2, if lit { 0xE5484D } else { c.dim }, 255);
                }
            }
            app.bar_hits.push((r, id, hit));
        };
        if !open {
            // The strip: what is next, and how many after it.
            let (label, first, total) = match (queue.first(), after.first()) {
                (Some(q), _) => (if paused { "held".to_string() } else { "next".to_string() }, q.clone(), queue.len() + after.len()),
                (None, Some((n, cmd))) => (format!("after {n}"), cmd.clone(), after.len()),
                _ => (String::new(), String::new(), 0),
            };
            let tx = area.x + pad;
            let ty = y + (row_h - sh) / 2;
            let end = self.canvas.text(&mut self.fonts.small, &label, tx as f32, ty, if paused && !queue.is_empty() { 0xE5484D } else { accent }, area);
            let more = if total > 1 { format!("  +{} more", total - 1) } else { String::new() };
            self.canvas.text_fit(&mut self.fonts.ui, &format!("{first}{more}"), end + px(10.0), y + (row_h - uh) / 2, area.x + area.w - end - pad * 2, c.text, area);
            self.bar_hits.push((Rect { x: area.x, y: area.y, w: area.w, h: area.h }, id, BarHit::Open));
            self.damage.push(area);
            return;
        }
        // What a command run now printed, its last lines.
        if let Some((cmd, text, status)) = &ran {
            let state = match status {
                None => "running…".to_string(),
                Some(0) => "done".to_string(),
                Some(n) => format!("exit {n}"),
            };
            let head = format!("ran  {cmd}");
            let end = self.canvas.text_fit(&mut self.fonts.small, &head, area.x + pad, y + (row_h - sh) / 2, area.w / 2, c.dim, area);
            self.canvas.text(&mut self.fonts.small, &state, (end + px(10.0)) as f32, y + (row_h - sh) / 2, if status.is_some_and(|s| s != 0) { 0xE5484D } else { c.faint }, area);
            button(self, area.x + area.w - pad - ib, y, BarHit::Dismiss, false);
            y += row_h;
            let lines: Vec<&str> = text.lines().collect();
            let shown = &lines[lines.len().saturating_sub(OUTPUT_SHOWN)..];
            for line in shown.iter().copied().chain(std::iter::once("").take(usize::from(shown.is_empty()))) {
                self.canvas.text_fit(&mut self.fonts.mono, line, area.x + pad, y + (line_h - mh) / 2, area.w - pad * 2, c.text, area);
                y += line_h;
            }
            y += px(6.0);
        }
        // What waits on another pane, each with remove.
        for (i, (n, cmd)) in after.iter().enumerate().take(QUEUE_SHOWN) {
            let label = format!("after {n}");
            let end = self.canvas.text(&mut self.fonts.small, &label, (area.x + pad) as f32, y + (row_h - sh) / 2, accent, area);
            let room = area.x + area.w - pad - ib - px(8.0) - (end + px(10.0));
            self.canvas.text_fit(&mut self.fonts.ui, cmd, end + px(10.0), y + (row_h - uh) / 2, room, c.text, area);
            button(self, area.x + area.w - pad - ib, y, BarHit::RemoveAfter(i), false);
            y += row_h;
        }
        // The queue, each with run now and remove.
        for (i, cmd) in queue.iter().enumerate().take(QUEUE_SHOWN) {
            let label = if i == 0 { if paused { "held" } else { "next" } } else { "then" };
            let ty = y + (row_h - sh) / 2;
            let end = self.canvas.text(&mut self.fonts.small, label, (area.x + pad) as f32, ty, if i == 0 && paused { 0xE5484D } else { c.faint }, area);
            let room = area.x + area.w - pad - 2 * ib - px(8.0) - (end + px(10.0));
            self.canvas.text_fit(&mut self.fonts.ui, cmd, end + px(10.0), y + (row_h - uh) / 2, room, c.text, area);
            button(self, area.x + area.w - pad - 2 * ib - px(4.0), y, BarHit::RunNow(i), true);
            button(self, area.x + area.w - pad - ib, y, BarHit::Remove(i), false);
            if i == 0 && paused {
                self.bar_hits.push((Rect { x: area.x, y, w: end - area.x + px(4.0), h: row_h }, id, BarHit::Resume));
            }
            y += row_h;
        }
        // The line typed into.
        let Some(b) = &self.bar else { return };
        let (field, selected) = (b.field.clone(), b.selected);
        let ty = y + (row_h - uh) / 2;
        let gx = self.canvas.text(&mut self.fonts.ui, "›", (area.x + pad) as f32, ty, accent, area) + px(8.0);
        let hint = if busy { "Enter queues" } else { "Enter runs" };
        let hw = self.fonts.small.width(hint);
        let field_clip = Rect { x: gx, y, w: area.x + area.w - pad - hw - px(12.0) - gx, h: row_h };
        if field.text.is_empty() {
            let place = if busy { "Queue a command to run when this one ends · / for commands" } else { "Run a command · / for commands" };
            self.canvas.text_fit(&mut self.fonts.ui, place, gx, ty, field_clip.w, c.faint, field_clip);
        }
        field.draw(&mut self.canvas, &mut self.fonts.ui, gx, ty, c.text, mix(accent, c.side, 140), true, field_clip);
        self.canvas.text(&mut self.fonts.small, hint, (area.x + area.w - pad - hw) as f32, y + (row_h - sh) / 2, c.faint, area);
        self.damage.push(area);
        // Where the slash commands rise from: drawn over everything, after
        // the panes, so that what the pane prints does not cover them.
        self.bar_list_at = Some((gx - px(12.0), area.y - px(4.0)));
        let _ = selected;
    }
}

impl App {
    /// The next queued command typed at the prompt, once one is due and
    /// the shell's line editor has the terminal (out of line mode).
    pub(super) fn queue_tick(&mut self, id: PaneId) {
        let Some(p) = self.panes.get_mut(&id) else { return };
        if !p.shell.queue_due {
            return;
        }
        if p.shell.paused || p.shell.queue.is_empty() || !p.shell.at_prompt {
            p.shell.queue_due = false;
            return;
        }
        let mut t: libc::termios = unsafe { std::mem::zeroed() };
        let reading = unsafe { libc::tcgetattr(p.pty.fd, &mut t) } == 0 && t.c_lflag & libc::ICANON == 0;
        if reading {
            p.shell.queue_due = false;
            self.run_next(id);
        }
    }
}

impl App {
    /// The pointer over a bar's buttons: the one under it lit, and the
    /// hand. True when it is on one.
    pub(super) fn bar_hover(&mut self, x: i32, y: i32) -> bool {
        let at = self.bar_hits.iter().position(|(r, _, _)| r.contains(x, y));
        if at != self.bar_lit {
            for i in [self.bar_lit, at].into_iter().flatten() {
                if let Some(p) = self.bar_hits.get(i).and_then(|(_, id, _)| self.panes.get_mut(id)) {
                    p.full = true;
                }
            }
            self.bar_lit = at;
            self.full = true;
            self.redraw();
        }
        if at.is_some() {
            if let Some(w) = &self.window {
                w.set_cursor(CursorIcon::Pointer);
            }
        }
        at.is_some()
    }
}

/// What a history word stands for: `!!` the last command, `!$` its last
/// word, `!?str` the latest with str in it, `!str` the latest starting
/// with str. None when it is not one, or nothing matches.
pub fn expand_bang(word: &str, history: &[String]) -> Option<String> {
    let rest = word.strip_prefix('!')?;
    let last = history.last();
    match rest {
        "" => None,
        "!" => last.cloned(),
        "$" => last.and_then(|l| l.split_whitespace().last()).map(str::to_string),
        _ => match rest.strip_prefix('?') {
            Some(s) => {
                let s = s.trim_end_matches('?');
                history.iter().rev().find(|h| h.contains(s)).cloned()
            }
            None => history.iter().rev().find(|h| h.starts_with(rest)).cloned(),
        },
    }
}

impl App {
    /// The word before the cursor expanded, if it is a history word that
    /// matches, and a space after it. True when it was.
    fn bar_expand(&mut self) -> bool {
        let Some(b) = &self.bar else { return false };
        let history = self.panes.get(&b.pane).map(|p| p.shell.history.clone()).unwrap_or_default();
        let Some(b) = &mut self.bar else { return false };
        let before: String = b.field.text[..b.field.cursor].iter().collect();
        let start = before.rfind(char::is_whitespace).map_or(0, |i| i + 1);
        let Some(with) = expand_bang(&before[start..], &history) else { return false };
        let from = before[..start].chars().count();
        let mut text: Vec<char> = b.field.text[..from].to_vec();
        text.extend(with.chars());
        text.push(' ');
        let cursor = text.len();
        text.extend_from_slice(&b.field.text[b.field.cursor..]);
        b.field.text = text;
        b.field.cursor = cursor;
        b.field.anchor = None;
        b.selected = 0;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::expand_bang;

    #[test]
    fn bangs() {
        let h: Vec<String> = ["cargo build --release", "git status", "ls -la /tmp"].iter().map(|s| s.to_string()).collect();
        assert_eq!(expand_bang("!!", &h).as_deref(), Some("ls -la /tmp"));
        assert_eq!(expand_bang("!$", &h).as_deref(), Some("/tmp"));
        assert_eq!(expand_bang("!car", &h).as_deref(), Some("cargo build --release"));
        assert_eq!(expand_bang("!?stat", &h).as_deref(), Some("git status"));
        assert_eq!(expand_bang("!nope", &h), None);
        assert_eq!(expand_bang("hello", &h), None);
    }
}

impl App {
    /// Whether the bar's slash commands are showing, so frames are drawn
    /// whole while they are.
    pub(super) fn bar_list_open(&self) -> bool {
        self.bar.as_ref().is_some_and(|b| b.field.text.first() == Some(&'/'))
    }

    /// The bar's slash commands, rising from its line, over everything.
    pub(super) fn draw_bar_list(&mut self) {
        let Some(b) = &self.bar else { return };
        let (text, selected) = (b.field.string(), b.selected);
        let Some((x, y)) = self.bar_list_at else { return };
        let rows = self.bar_rows_for(&text);
        if !rows.is_empty() {
            let first = selected.saturating_sub(7);
            self.draw_command_list(&rows, selected, first, x, y, y, true);
        }
    }
}

impl App {
    /// A pane named as @ names it: N, the Nth pane of this workspace, or
    /// W.P, pane P of workspace W, counted from 1 as captions show them.
    pub(super) fn pane_by_number(&self, name: &str) -> Option<PaneId> {
        let (w, p) = match name.split_once('.') {
            Some((w, p)) => (w.parse::<usize>().ok()?.checked_sub(1)?, p),
            None => (self.current, name),
        };
        let mut ids = Vec::new();
        self.workspaces.get(w)?.root.panes(&mut ids);
        ids.get(p.parse::<usize>().ok()?.checked_sub(1)?).copied()
    }

    /// The commands waiting on pane `done`'s command, now it has ended:
    /// into their panes' queues, held if it failed.
    pub(super) fn release_after(&mut self, done: PaneId, failed: bool) {
        let waiting: Vec<PaneId> = self.panes.iter().filter(|(_, p)| p.shell.after.iter().any(|(t, _)| *t == done)).map(|(id, _)| *id).collect();
        for id in waiting {
            let Some(p) = self.panes.get_mut(&id) else { continue };
            let (go, stay): (Vec<_>, Vec<_>) = std::mem::take(&mut p.shell.after).into_iter().partition(|(t, _)| *t == done);
            p.shell.after = stay;
            p.shell.queue.extend(go.into_iter().map(|(_, cmd)| cmd));
            if failed {
                p.shell.paused = true;
            }
            p.shell.queue_due = true;
            p.full = true;
            self.queue_tick(id);
        }
        self.relayout();
    }
}

impl App {
    /// A pane's number as @ takes it: N in this workspace, W.P elsewhere.
    fn pane_number(&self, id: PaneId) -> String {
        for (wi, ws) in self.workspaces.iter().enumerate() {
            let mut ids = Vec::new();
            ws.root.panes(&mut ids);
            if let Some(n) = ids.iter().position(|p| *p == id) {
                return if wi == self.current { format!("{}", n + 1) } else { format!("{}.{}", wi + 1, n + 1) };
            }
        }
        "?".into()
    }
}
