//! What jterm makes of the shell's marks (OSC 133): each command as a
//! block, with a bar beside it in green, red, or amber while it runs, and
//! its exit status and time when that is worth saying; Ctrl Shift Up and
//! Down from prompt to prompt; a notice when a long command ends where you
//! cannot see it; and the command line lifted into an editor of its own.

use super::*;
use crate::vt::ShellEvent;

/// A command run, by the rows it took, counted from the top of the
/// scrollback.
#[derive(Clone, Debug)]
pub struct Block {
    pub row: u64,
    pub end: u64,
    pub exit: Option<i32>,
    pub took: Duration,
    /// Its command line.
    pub command: String,
}

#[derive(Default)]
pub struct Shell {
    /// The shell has marked its prompts: there is something to build on.
    pub marked: bool,
    pub at_prompt: bool,
    /// Where typing began after the prompt: the cursor at the first key.
    pub input_at: Option<(u16, u64)>,
    /// What is on the command line, as far as the keys
    /// typed tell; None once a key does what cannot be followed (history,
    /// completion, moving along the line).
    pub line: Option<String>,
    /// The line as it was when Enter was pressed, when it was known.
    pub entered: Option<String>,
    /// The shell expanded a history word on the line (!!, !str): what is
    /// on it is what the screen shows, not what was typed.
    pub expanded: bool,
    /// Commands to run, in turn, as each before them ends; held when one
    /// fails.
    pub queue: Vec<String>,
    pub paused: bool,
    /// A prompt came: the next in the queue goes once the shell's line
    /// editor is reading, so that it is not echoed twice.
    pub queue_due: bool,
    /// What the pane ran, oldest first: at its prompt, from its command
    /// line, from its queue.
    pub history: Vec<String>,
    /// A command run now, beside the shell: its line, what it printed so
    /// far, and its exit status once it ended.
    pub ran: Option<(String, String, Option<i32>)>,
    /// When the running job was last sent SIGTERM, to send SIGKILL next.
    pub killed_at: Option<Instant>,
    /// /notify: the command running, or the next, told of when it ends.
    pub notify: bool,
    /// /restart sent Ctrl C: the command's failing that way holds nothing.
    pub restarting: bool,
    /// Commands waiting for another pane's command to end (@2 cmd): then
    /// they join this pane's queue.
    pub after: Vec<(PaneId, String)>,
    pub prompt_row: u64,
    pub running: Option<(Instant, u64, String)>,
    pub blocks: Vec<Block>,
}

/// A command line being edited in a box of its own.
pub struct Editor {
    pub pane: PaneId,
    pub lines: Vec<Vec<char>>,
    pub row: usize,
    pub col: usize,
    /// What is made goes to the pane's command line, not its prompt.
    pub into_bar: bool,
    /// Where a selection starts, row and column; it ends at the cursor.
    pub anchor: Option<(usize, usize)>,
    /// Where its text was drawn: the top left of row 0, column 0.
    pub origin: (i32, i32),
    /// A drag selecting; its last clicks, for double and triple ones.
    pub drag: bool,
    pub clicks: Option<(Instant, u8)>,
    /// Its Run button, where it was drawn, and whether the pointer is on it.
    pub run_at: Rect,
    pub run_lit: bool,
}

impl App {
    /// The pane's cursor, counted from the top of the scrollback.
    fn cursor_row(p: &Pane) -> (u16, u64) {
        p.term.cursor_now()
    }

    /// The shell's marks since last time, made into blocks and notices.
    pub(super) fn shell_events(&mut self, id: PaneId) {
        let shown = self.focused_window && (self.ws().root.contains(id) && !self.roam_shown || self.roam == Some(id) && self.roam_shown || self.popout_of(id).is_some());
        let place = self.pane_label(id);
        let Some(p) = self.panes.get_mut(&id) else { return };
        let events = p.term.take_events();
        if events.is_empty() {
            return;
        }
        let (_, row) = Self::cursor_row(p);
        let mut notices: Vec<(String, String, u32)> = Vec::new();
        let mut prompted = false;
        // Its command ended, and whether it failed: what waits on it goes.
        let mut ended: Option<bool> = None;
        // What /notify asked to be told, whatever the time it took.
        let mut told: Vec<(String, String, u32)> = Vec::new();
        for e in events {
            match e {
                ShellEvent::Prompt => {
                    p.shell.marked = true;
                    prompted = true;
                    p.shell.at_prompt = true;
                    p.shell.input_at = None;
                    p.shell.line = Some(String::new());
                    p.shell.expanded = false;
                    p.shell.prompt_row = row;
                }
                ShellEvent::Input => {}
                ShellEvent::Output(cmd) if p.shell.at_prompt => {
                    p.shell.at_prompt = false;
                    let start = p.shell.input_at.map_or(p.shell.prompt_row, |(_, r)| r);
                    // The command as the shell sent it, else as it was typed, else
                    // as it reads on the screen.
                    let entered = p.shell.entered.take().filter(|e| !e.trim().is_empty());
                    let line = if !cmd.is_empty() { cmd } else { entered.unwrap_or_else(|| Self::typed(p, start, row)) };
                    p.shell.running = Some((Instant::now(), start, line));
                    // Its caption shows the restart button while it runs.
                    p.full = true;
                }
                ShellEvent::Output(_) => {}
                ShellEvent::End(exit) => {
                    let Some((started, start, line)) = p.shell.running.take() else { continue };
                    let took = started.elapsed();
                    Self::pins_ended(p, &line, exit);
                    if std::mem::take(&mut p.shell.notify) {
                        let how = match exit {
                            Some(0) | None => "done".to_string(),
                            Some(n) => format!("failed, exit {n}"),
                        };
                        let color = if exit.is_some_and(|e| e != 0) { 0xE5484D } else { 0x30A46C };
                        told.push((place.clone(), format!("{} · {how} in {}", line.trim(), human(took)), color));
                        p.full = true;
                    }
                    ended = Some(exit.is_some_and(|e| e != 0));
                    // A failed command holds what is queued behind it.
                    let restarting = std::mem::take(&mut p.shell.restarting);
                    if exit.is_some_and(|e| e != 0) && !p.shell.queue.is_empty() && !restarting {
                        p.shell.paused = true;
                    }
                    // The new prompt is drawn by now: the command's rows end
                    // the row before it.
                    let end = row.saturating_sub(1).max(start);
                    p.shell.blocks.push(Block { row: start, end, exit, took, command: line.trim().to_string() });
                    // Every command the pane ran is in its history, however it
                    // was typed: at the prompt, in its command line, queued.
                    let cmd = line.trim().to_string();
                    if !cmd.is_empty() {
                        p.shell.history.retain(|h| *h != cmd);
                        p.shell.history.push(cmd);
                        if p.shell.history.len() > 500 {
                            p.shell.history.remove(0);
                        }
                    }
                    if p.shell.blocks.len() > 2000 {
                        p.shell.blocks.remove(0);
                    }
                    p.full = true;
                    if took >= Duration::from_secs(10) && !shown && !line.trim().is_empty() && told.is_empty() {
                        let how = match exit {
                            Some(0) | None => "done".to_string(),
                            Some(n) => format!("failed, exit {n}"),
                        };
                        let color = if exit.is_some_and(|e| e != 0) { 0xE5484D } else { 0x30A46C };
                        notices.push((place.clone(), format!("{} · {how} in {}", line.trim(), human(took)), color));
                        p.activity = true;
                    }
                }
            }
        }
        for (title, body, color) in told.into_iter().chain(notices) {
            self.toast(&title, &body, Some(color), Some(id));
        }
        if let Some(failed) = ended {
            self.release_after(id, failed);
        }
        // Back at a prompt: the next queued command is due.
        if prompted {
            if let Some(p) = self.panes.get_mut(&id) {
                p.shell.queue_due = true;
            }
        }
    }

    /// The text typed at a prompt, from `from` to the row before `to`.
    fn typed(p: &mut Pane, from: u64, to: u64) -> String {
        let x = p.shell.input_at.map_or(0, |(x, _)| x);
        let last = to.saturating_sub(1).max(from);
        p.term.select_rows((x, from), (p.cols.saturating_sub(1), last));
        let text = p.term.selection_text().unwrap_or_default();
        p.term.clear_selection();
        text.trim().to_string()
    }

    fn pane_label(&self, id: PaneId) -> String {
        let name = self.workspaces.iter().find(|w| w.root.contains(id)).map(|w| w.name.clone()).unwrap_or_default();
        let caption = self.panes.get(&id).map(|p| p.caption.clone()).unwrap_or_default();
        if name.is_empty() { caption } else { format!("{name}: {caption}") }
    }

    /// A key is about to go to pane `id`: the first after a prompt marks
    /// where typing began.
    pub(super) fn note_typing(&mut self, id: PaneId) {
        if let Some(p) = self.panes.get_mut(&id) {
            if p.shell.at_prompt && p.shell.input_at.is_none() {
                p.shell.input_at = Some(Self::cursor_row(p));
            }
        }
    }

    /// The bars beside the commands in view, and the label of a command
    /// that failed or took a while; drawn after the rows, each time, solid.
    pub(super) fn draw_blocks(&mut self, id: PaneId) {
        let (cw, ch) = (self.fonts.cell_w, self.fonts.cell_h);
        let scale = self.scale;
        let px = |v: f32| (v * scale).round() as i32;
        let Some(p) = self.panes.get(&id) else { return };
        if !p.shell.marked || !self.config.command_bars {
            return;
        }
        let (_, offset, len) = p.term.scrollbar();
        let (content, rect, bg) = (p.content, p.rect, self.cur_theme.bg);
        let cursor = Self::cursor_row(p).1;
        let mut bars: Vec<(u64, u64, u32, Option<String>)> = p
            .shell
            .blocks
            .iter()
            .filter(|b| b.end >= offset && b.row < offset + len)
            .map(|b| {
                let color = if b.exit.is_some_and(|e| e != 0) { 0xE5484D } else { 0x30A46C };
                let label = match (b.exit, b.took.as_secs_f32()) {
                    (Some(e), t) if e != 0 => Some(format!("exit {e} · {}", human(Duration::from_secs_f32(t)))),
                    (_, t) if t >= 2.0 => Some(human(Duration::from_secs_f32(t))),
                    _ => None,
                };
                (b.row, b.end, color, label)
            })
            .collect();
        if let Some((started, start, _)) = &p.shell.running {
            bars.push((*start, cursor, 0xFFC53D, (started.elapsed() >= Duration::from_secs(2)).then(|| format!("running · {}", human(started.elapsed())))));
        }
        let ui = &mut self.fonts.small;
        for (row, end, color, label) in bars {
            let top = content.y + (row.max(offset) - offset) as i32 * ch;
            let bottom = content.y + ((end.min(offset + len - 1) - offset) as i32 + 1) * ch;
            let bar = Rect { x: rect.x + px(4.0), y: top + 1, w: px(3.0).max(2), h: (bottom - top - 2).max(1) };
            self.canvas.round_fill(bar, px(2.0), color, 255);
            self.damage.push(bar);
            // The label on the command's own row, at the right, on a chip.
            if let (Some(text), true) = (label, row >= offset) {
                let w = ui.width(&text) + px(10.0);
                let r = Rect { x: content.x + content.w - w, y: top + (ch - ui.height - px(4.0)) / 2, w, h: ui.height + px(4.0) };
                self.canvas.round_fill(r, px(4.0), mix(bg, color, 60), 255);
                self.canvas.text(ui, &text, (r.x + px(5.0)) as f32, r.y + px(2.0), mix(color, 0xFFFFFF, 90), r);
                self.damage.push(r);
            }
        }
        let _ = cw;
    }

    /// Ctrl Shift Up and Down: the prompt before or after.
    pub(super) fn jump_prompt(&mut self, by: isize) {
        if let Some(p) = self.focused().and_then(|f| self.panes.get_mut(&f)) {
            p.term.jump_prompt(by);
            p.full = true;
            self.redraw();
        }
    }

    /// The output of the command under the pointer, selected and copied.
    pub(super) fn copy_output_at(&mut self, at: (i32, i32)) {
        let Some((id, cell)) = self.pane_at(at.0, at.1) else { return };
        let Some(p) = self.panes.get_mut(&id) else { return };
        if p.term.select_output_at(cell.0, cell.1) {
            p.full = true;
            self.copy(false);
            self.copy(true);
            self.redraw();
        }
    }

    /// The command whose rows are at a point of the window: the last one
    /// to start at or above it and not end before it.
    pub(super) fn command_at(&self, at: (i32, i32)) -> Option<String> {
        let (id, cell) = self.pane_at(at.0, at.1)?;
        let p = self.panes.get(&id)?;
        let (_, offset, _) = p.term.scrollbar();
        let row = offset + cell.1 as u64;
        // The command running, from its first row to the cursor, else one
        // that ended.
        let running = p.shell.running.as_ref().filter(|(_, start, _)| *start <= row && row <= p.term.cursor_now().1).map(|(_, _, line)| line.clone());
        running.or_else(|| p.shell.blocks.iter().rev().find(|b| b.row <= row && row <= b.end).map(|b| b.command.clone())).filter(|c| !c.trim().is_empty())
    }

    /// The command line under a point, to the clipboard.
    pub(super) fn copy_command_at(&mut self, at: (i32, i32)) {
        if let Some(cmd) = self.command_at(at) {
            self.clip_set(cmd, false);
        }
    }

    /// The command line at a pane's prompt: as followed key by key, else
    /// as it reads on the screen.
    pub(super) fn prompt_line(&mut self, id: PaneId) -> String {
        let Some(p) = self.panes.get_mut(&id) else { return String::new() };
        if let Some(line) = p.shell.line.clone().filter(|_| !p.shell.expanded) {
            return line;
        }
        match p.shell.input_at {
            Some((x, row)) => {
                let (_, cur) = Self::cursor_row(p);
                p.term.select_rows((x, row), (p.cols.saturating_sub(1), cur));
                let t = p.term.selection_text().unwrap_or_default();
                p.term.clear_selection();
                t.replace('\n', "")
            }
            None => String::new(),
        }
    }

    /// Ctrl Shift E: the command line being typed, in an editor of its own.
    pub(super) fn open_editor(&mut self) {
        let Some(id) = self.focused() else { return };
        let text = self.prompt_line(id);
        self.open_editor_with(id, &text, false);
    }

    /// A command in the editor, broken where its options and pipes start;
    /// what is made of it goes back to the prompt, or with `into_bar` to
    /// the pane's command line.
    pub(super) fn open_editor_with(&mut self, id: PaneId, text: &str, into_bar: bool) {
        let mut lines: Vec<Vec<char>> = vec![Vec::new()];
        let words: Vec<&str> = text.trim().split(' ').collect();
        for (i, w) in words.iter().enumerate() {
            let breaks = i > 0 && (w.starts_with('-') || ["|", "&&", "||", ";"].contains(w)) && lines.last().is_some_and(|l| l.len() > 12);
            if breaks {
                lines.push(Vec::new());
            } else if i > 0 {
                lines.last_mut().unwrap().push(' ');
            }
            lines.last_mut().unwrap().extend(w.chars());
        }
        let row = lines.len() - 1;
        let col = lines[row].len();
        self.editor = Some(Editor { pane: id, lines, row, col, into_bar, anchor: None, origin: (0, 0), drag: false, clicks: None, run_at: Rect::default(), run_lit: false });
        self.full = true;
        self.redraw();
    }

    /// The edited line back where it came from: at the prompt, in place of
    /// what was there, or in the pane's command line. Run, with `run`.
    pub(super) fn submit_editor(&mut self, run: bool) {
        let Some(e) = self.editor.take() else { return };
        let line = e.lines.iter().map(|l| l.iter().collect::<String>().trim().to_string()).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
        if e.into_bar {
            if self.bar.as_ref().is_none_or(|b| b.pane != e.pane) {
                self.bar = None;
                self.set_focus(e.pane);
                self.toggle_bar();
            }
            if let Some(b) = &mut self.bar {
                b.field = crate::field::TextField::new(&line);
                b.selected = 0;
            }
            if run {
                self.bar_submit();
            }
            self.full = true;
            return self.relayout();
        }
        if let Some(p) = self.panes.get_mut(&e.pane) {
            // To the end of the line, and all of it taken away (Ctrl E,
            // Ctrl U), then the new line typed.
            let mut bytes = vec![0x05, 0x15];
            bytes.extend_from_slice(line.as_bytes());
            if run {
                bytes.push(b'\r');
                p.shell.entered = Some(line.clone());
            }
            p.pty.write(&bytes);
            p.shell.line = if run { None } else { Some(line) };
        }
        self.full = true;
        self.redraw();
    }

    pub(super) fn editor_key(&mut self, event: &winit::event::KeyEvent) {
        let PhysicalKey::Code(code) = event.physical_key else { return };
        let (ctrl, shift) = (self.mods.control_key(), self.mods.shift_key());
        match code {
            KeyCode::Escape => {
                self.editor = None;
                self.full = true;
                return self.redraw();
            }
            KeyCode::Enter | KeyCode::NumpadEnter if ctrl => return self.submit_editor(true),
            KeyCode::Enter | KeyCode::NumpadEnter if shift => return self.submit_editor(false),
            _ => {}
        }
        let pasted = if ctrl && code == KeyCode::KeyV { self.clip_get(false) } else { None };
        let copied: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
        let mods = self.mods;
        if let Some(e) = &mut self.editor {
            let copied = &copied;
            e.key(code, mods, event.text.as_deref(), &mut || pasted.clone(), &mut |t| *copied.borrow_mut() = Some(t));
        }
        if let Some(t) = copied.into_inner() {
            self.clip_set(t, false);
        }
        self.full = true;
        self.redraw();
    }

    pub(super) fn draw_editor(&mut self) {
        let Some(e) = self.editor.as_ref() else { return };
        let Some(prect) = self.panes.get(&e.pane).map(|p| p.rect) else { return };
        let c = self.cur_theme.chrome();
        let (cw, ch) = (self.fonts.cell_w, self.fonts.cell_h);
        let scale = self.scale;
        let px = |v: f32| (v * scale).round() as i32;
        let (lines, row, col, sel) = (e.lines.clone(), e.row, e.col, e.selection());
        let pad = px(14.0);
        let head = self.fonts.ui.height + px(18.0);
        // The footer holds the Run button, with room above and below it.
        let foot = self.fonts.ui.height + px(10.0) + 2 * px(12.0);
        let w = (prect.w - px(40.0)).min(cw * 100 + 2 * pad + px(28.0)).max(px(300.0));
        let h = (head + lines.len() as i32 * ch + 2 * pad + foot).min(prect.h - px(20.0));
        let r = Rect { x: prect.x + (prect.w - w) / 2, y: prect.y + (prect.h - h) / 3, w, h };
        self.canvas.panel(r, px(10.0), c.panel, c.edge);
        self.canvas.text(&mut self.fonts.ui, "Edit command", (r.x + pad) as f32, r.y + px(9.0), c.text, r);
        let body = Rect { x: r.x + 1, y: r.y + head, w: r.w - 2, h: r.h - head - foot };
        self.canvas.fill(body, self.cur_theme.bg);
        let gutter = px(28.0);
        let origin = (body.x + pad + gutter, body.y + pad);
        if let Some(e) = &mut self.editor {
            e.origin = origin;
        }
        let sel_color = mix(self.cur_accent, self.cur_theme.bg, 150);
        for (i, line) in lines.iter().enumerate() {
            let y = body.y + pad + i as i32 * ch;
            if y + ch > body.y + body.h {
                break;
            }
            let n = format!("{}", i + 1);
            let sh = self.fonts.small.height;
            // The part of this line selected, lit under its text.
            if let Some(((r0, c0), (r1, c1))) = sel {
                if (r0..=r1).contains(&i) {
                    let from = if i == r0 { c0 } else { 0 };
                    let to = if i == r1 { c1 } else { line.len() + 1 };
                    let x0 = origin.0 + from as i32 * cw;
                    let w = ((to.saturating_sub(from)) as i32 * cw).min(body.x + body.w - pad - x0);
                    if w > 0 {
                        self.canvas.fill(Rect { x: x0, y, w, h: ch }, sel_color);
                    }
                }
            }
            self.canvas.text(&mut self.fonts.small, &n, (body.x + pad) as f32, y + (ch - sh) / 2, c.faint, body);
            for (k, chr) in line.iter().enumerate() {
                let x = body.x + pad + gutter + k as i32 * cw;
                if x + cw > body.x + body.w - pad {
                    break;
                }
                if *chr != ' ' {
                    let g = self.fonts.glyph(*chr, 0);
                    self.canvas.glyph(g, x, y, self.cur_theme.fg, body);
                }
            }
            if i == row {
                self.canvas.fill(Rect { x: body.x + pad + gutter + col as i32 * cw, y, w: px(2.0).max(2), h: ch }, self.cur_accent);
            }
        }
        let hint = "Enter  new line      Shift Enter  back to the prompt      Esc  cancel";
        // Run, at the bottom right, as Ctrl Enter.
        let lit = self.editor.as_ref().is_some_and(|e| e.run_lit);
        let label = "Run";
        let keys = "Ctrl Enter";
        let (uh, smh) = (self.fonts.ui.height, self.fonts.small.height);
        let bh = uh + px(10.0);
        let bw = self.fonts.ui.width(label) + px(14.0) + self.fonts.small.width(keys) + px(24.0);
        let run = Rect { x: r.x + r.w - pad - bw, y: r.y + r.h - foot + (foot - bh) / 2, w: bw, h: bh };
        let accent = self.cur_accent;
        self.canvas.round_fill(run, px(7.0), if lit { mix(accent, 0xFFFFFF, 30) } else { accent }, 255);
        let fg = crate::canvas::on(accent);
        let tx = self.canvas.text(&mut self.fonts.ui, label, (run.x + px(12.0)) as f32, run.y + (bh - uh) / 2, fg, run);
        self.canvas.text(&mut self.fonts.small, keys, (tx + px(10.0)) as f32, run.y + (bh - smh) / 2, mix(accent, fg, 170), run);
        if let Some(e) = &mut self.editor {
            e.run_at = run;
        }
        let hint_h = self.fonts.small.height;
        self.canvas.text(&mut self.fonts.small, hint, (r.x + pad) as f32, r.y + r.h - foot + (foot - hint_h) / 2, c.dim, r);
        self.damage.push(Rect { x: 0, y: 0, w: self.canvas.width, h: self.canvas.height });
    }
}

/// A time as a person says it: 850 ms, 2.3 s, 4m 05s, 1h 02m.
pub fn human(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 1.0 {
        format!("{} ms", d.as_millis())
    } else if s < 60.0 {
        format!("{s:.1} s")
    } else if s < 3600.0 {
        format!("{}m {:02}s", (s / 60.0) as u64, s as u64 % 60)
    } else {
        format!("{}h {:02}m", (s / 3600.0) as u64, (s as u64 % 3600) / 60)
    }
}

impl App {
    /// The command under a point, run again: at the prompt, or queued
    /// behind what the pane is running.
    pub(super) fn rerun_at(&mut self, at: (i32, i32)) {
        let (Some(cmd), Some((id, _))) = (self.command_at(at), self.pane_at(at.0, at.1)) else { return };
        self.run_or_queue(id, cmd);
    }

    /// The command under a point, in the editor to change first; what is
    /// made goes to the prompt, or to the pane's command line to queue
    /// when it is busy.
    pub(super) fn edit_command_at(&mut self, at: (i32, i32)) {
        let (Some(cmd), Some((id, _))) = (self.command_at(at), self.pane_at(at.0, at.1)) else { return };
        let busy = !self.pane_waiting(id);
        self.open_editor_with(id, &cmd, busy);
    }
}
