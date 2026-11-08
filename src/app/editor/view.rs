//! The long-command editor drawn: a box over its pane, the command in it
//! colored by what each part is, line numbers, the selection and the
//! cursor, and a Run button at its foot.

use super::syntax;
use super::*;

impl App {
    /// Every editor on screen, over its pane: when the frame is drawn
    /// whole, or its pane was drawn again under it (`drawn`).
    pub(in crate::app) fn draw_editors(&mut self, full: bool, drawn: &[PaneId]) {
        // An editor whose pane ended goes with it.
        let panes = &self.panes;
        self.editors.retain(|e| panes.contains_key(&e.pane));
        let focused = self.focused();
        for i in 0..self.editors.len() {
            let (pane, shown) = (self.editors[i].pane, self.editor_shown(&self.editors[i]));
            if shown && (full || drawn.contains(&pane)) {
                self.draw_editor(i, focused == Some(pane));
            }
        }
    }

    fn draw_editor(&mut self, i: usize, active: bool) {
        let e = &self.editors[i];
        let Some(prect) = self.panes.get(&e.pane).map(|p| p.rect) else { return };
        let c = self.cur_theme.chrome();
        let t = self.cur_theme;
        let (cw, ch) = (self.fonts.cell_w, self.fonts.cell_h);
        let scale = self.scale;
        let px = |v: f32| (v * scale).round() as i32;
        let (lines, row, col, sel, tokens) = (e.lines.clone(), e.row, e.col, e.selection(), e.tokens());
        let lit = e.run_lit;
        let pad = px(14.0);
        let head = self.fonts.ui.height + px(18.0);
        // The footer holds the Run button, with room above and below it.
        let foot = self.fonts.ui.height + px(10.0) + 2 * px(12.0);
        let w = (prect.w - px(40.0)).min(cw * 100 + 2 * pad + px(28.0)).max(px(300.0)).min(prect.w);
        let h = (head + lines.len() as i32 * ch + 2 * pad + foot).min(prect.h - px(20.0));
        let r = Rect { x: prect.x + (prect.w - w) / 2, y: prect.y + (prect.h - h) / 3, w, h };
        self.canvas.panel(r, px(10.0), c.panel, c.edge);
        self.canvas.text(&mut self.fonts.ui, "Edit command", (r.x + pad) as f32, r.y + px(9.0), if active { c.text } else { c.dim }, r);
        let body = Rect { x: r.x + 1, y: r.y + head, w: r.w - 2, h: r.h - head - foot };
        self.canvas.fill(body, t.bg);
        let gutter = px(28.0);
        let origin = (body.x + pad + gutter, body.y + pad);
        let sel_color = mix(self.cur_accent, t.bg, 150);
        // Not the pane you are in: its words faded, no cursor.
        let fade = |color: u32| if active { color } else { mix(t.bg, color, 150) };
        for (n, line) in lines.iter().enumerate() {
            let y = body.y + pad + n as i32 * ch;
            if y + ch > body.y + body.h {
                break;
            }
            let sh = self.fonts.small.height;
            // The part of this line selected, lit under its text.
            if let Some(((r0, c0), (r1, c1))) = sel {
                if (r0..=r1).contains(&n) {
                    let from = if n == r0 { c0 } else { 0 };
                    let to = if n == r1 { c1 } else { line.len() + 1 };
                    let x0 = origin.0 + from as i32 * cw;
                    let w = ((to.saturating_sub(from)) as i32 * cw).min(body.x + body.w - pad - x0);
                    if w > 0 {
                        self.canvas.fill(Rect { x: x0, y, w, h: ch }, sel_color);
                    }
                }
            }
            self.canvas.text(&mut self.fonts.small, &format!("{}", n + 1), (body.x + pad) as f32, y + (ch - sh) / 2, c.faint, body);
            // Each character in its token's color; plain text where no
            // token is.
            let mut colors = vec![t.fg; line.len()];
            for tok in tokens.get(n).into_iter().flatten() {
                let color = syntax::color(tok.kind, t);
                for slot in colors.iter_mut().take(tok.end.min(line.len())).skip(tok.start) {
                    *slot = color;
                }
            }
            for (k, chr) in line.iter().enumerate() {
                let x = origin.0 + k as i32 * cw;
                if x + cw > body.x + body.w - pad {
                    break;
                }
                if *chr != ' ' {
                    let g = self.fonts.glyph(*chr, 0);
                    self.canvas.glyph(g, x, y, fade(colors[k]), body);
                }
            }
            if active && n == row {
                self.canvas.fill(Rect { x: origin.0 + col as i32 * cw, y, w: px(2.0).max(2), h: ch }, self.cur_accent);
            }
        }
        let hint = "Enter  new line      Shift Enter  back to the prompt      Esc  cancel";
        // Run, at the bottom right, as Ctrl Enter.
        let label = "Run";
        let keys = "Ctrl Enter";
        let (uh, smh) = (self.fonts.ui.height, self.fonts.small.height);
        let bh = uh + px(10.0);
        let bw = self.fonts.ui.width(label) + px(14.0) + self.fonts.small.width(keys) + px(24.0);
        let run = Rect { x: r.x + r.w - pad - bw, y: r.y + r.h - foot + (foot - bh) / 2, w: bw, h: bh };
        let accent = if active { self.cur_accent } else { mix(c.panel, self.cur_accent, 120) };
        self.canvas.round_fill(run, px(7.0), if lit { mix(accent, 0xFFFFFF, 30) } else { accent }, 255);
        let fg = crate::canvas::on(accent);
        let tx = self.canvas.text(&mut self.fonts.ui, label, (run.x + px(12.0)) as f32, run.y + (bh - uh) / 2, fg, run);
        self.canvas.text(&mut self.fonts.small, keys, (tx + px(10.0)) as f32, run.y + (bh - smh) / 2, mix(accent, fg, 170), run);
        let hint_h = self.fonts.small.height;
        self.canvas.text(&mut self.fonts.small, hint, (r.x + pad) as f32, r.y + r.h - foot + (foot - hint_h) / 2, c.dim, Rect { w: (run.x - r.x - pad).max(0), ..r });
        let e = &mut self.editors[i];
        e.at = r;
        e.origin = origin;
        e.run_at = run;
        self.damage.push(r);
    }
}
