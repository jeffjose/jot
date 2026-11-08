//! The long-command editor drawn: a box over its pane, the command in it
//! colored by what each part is (in the scheme Settings picks), line
//! numbers, the selection and the cursor, scrolled to keep the cursor in
//! sight; a close button at its top right and a Run button at its foot.

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
        let Some(prect) = self.panes.get(&self.editors[i].pane).map(|p| p.rect) else { return };
        let c = self.cur_theme.chrome();
        let scheme = syntax::scheme(&self.config.editor_colors, self.cur_theme);
        let (cw, ch) = (self.fonts.cell_w, self.fonts.cell_h);
        let scale = self.scale;
        let px = |v: f32| (v * scale).round() as i32;
        let pad = px(14.0);
        let head = self.fonts.ui.height + px(18.0);
        // The footer holds the Run button, with room above and below it.
        let foot = self.fonts.ui.height + px(10.0) + 2 * px(12.0);
        let gutter = px(28.0);
        let n_lines = self.editors[i].lines.len() as i32;
        // Over the pane's text, its caption left in sight.
        let cap = self.caption_h();
        let room = Rect { x: prect.x, y: prect.y + cap, w: prect.w, h: (prect.h - cap).max(0) };
        let w = (room.w - px(40.0)).min(cw * 100 + 2 * pad + gutter).max(px(300.0)).min(room.w);
        let h = (head + n_lines * ch + 2 * pad + foot).min(room.h - px(20.0)).max(head + ch + 2 * pad + foot).min(room.h);
        let r = Rect { x: room.x + (room.w - w) / 2, y: room.y + ((room.h - h) / 3).max(0), w, h };
        let body = Rect { x: r.x + 1, y: r.y + head, w: r.w - 2, h: r.h - head - foot };
        // Where the text shows, and how much of it fits there.
        let text_at = Rect { x: body.x + pad + gutter, y: body.y + pad, w: (body.w - 2 * pad - gutter).max(cw), h: (body.h - 2 * pad).max(ch) };
        let fits = ((text_at.h / ch).max(1) as usize, (text_at.w / cw).max(1) as usize);
        let e = &mut self.editors[i];
        e.fits = fits;
        e.text_at = text_at;
        if std::mem::take(&mut e.follow_next) {
            e.follow();
        }
        let (scroll_r, scroll_c) = e.scroll;
        // Row 0, column 0, where they would be, scrolled: for the mouse.
        e.origin = (text_at.x - scroll_c as i32 * cw, text_at.y - scroll_r as i32 * ch);
        let (lines, row, col, sel, tokens, run_lit, close_lit) = (e.lines.clone(), e.row, e.col, e.selection(), e.tokens(), e.run_lit, e.close_lit);

        self.canvas.panel(r, px(10.0), c.panel, c.edge);
        self.canvas.text(&mut self.fonts.ui, "Edit command", (r.x + pad) as f32, r.y + px(9.0), if active { c.text } else { c.dim }, r);
        // Close, at the top right.
        let b = self.fonts.ui.height + px(8.0);
        let close = Rect { x: r.x + r.w - px(8.0) - b, y: r.y + (head - b) / 2, w: b, h: b };
        if close_lit {
            self.canvas.round_fill(close, px(6.0), c.hover, 255);
        }
        self.canvas.icon_in(crate::canvas::icons::CLOSE, close, px(14.0), if close_lit { c.text } else { c.dim });
        self.canvas.fill(body, scheme.bg);
        let sel_color = mix(self.cur_accent, scheme.bg, 150);
        // Not the pane you are in: its words faded, no cursor.
        let fade = |color: u32| if active { color } else { mix(scheme.bg, color, 150) };
        let numbers = mix(scheme.bg, scheme.fg, 90);
        let sh = self.fonts.small.height;
        for (k, n) in (scroll_r..lines.len()).take(fits.0).enumerate() {
            let line = &lines[n];
            let y = text_at.y + k as i32 * ch;
            // The part of this line selected, lit under its text.
            if let Some(((r0, c0), (r1, c1))) = sel {
                if (r0..=r1).contains(&n) {
                    let from = (if n == r0 { c0 } else { 0 }).max(scroll_c);
                    let to = if n == r1 { c1 } else { line.len() + 1 };
                    let x0 = text_at.x + (from - scroll_c) as i32 * cw;
                    let w = ((to.saturating_sub(from)) as i32 * cw).min(text_at.x + text_at.w - x0);
                    if w > 0 {
                        self.canvas.fill(Rect { x: x0, y, w, h: ch }, sel_color);
                    }
                }
            }
            self.canvas.text(&mut self.fonts.small, &format!("{}", n + 1), (body.x + pad) as f32, y + (ch - sh) / 2, numbers, body);
            // Each character in its token's color; plain text where no
            // token is.
            let mut colors = vec![scheme.fg; line.len()];
            for tok in tokens.get(n).into_iter().flatten() {
                let color = scheme.color(tok.kind);
                for slot in colors.iter_mut().take(tok.end.min(line.len())).skip(tok.start) {
                    *slot = color;
                }
            }
            for (k, chr) in line.iter().enumerate().skip(scroll_c).take(fits.1) {
                let x = text_at.x + (k - scroll_c) as i32 * cw;
                if *chr != ' ' {
                    let g = self.fonts.glyph(*chr, 0);
                    self.canvas.glyph(g, x, y, fade(colors[k]), text_at);
                }
            }
            if active && n == row && col >= scroll_c && col <= scroll_c + fits.1 {
                self.canvas.fill(Rect { x: text_at.x + (col - scroll_c) as i32 * cw, y, w: px(2.0).max(2), h: ch }, self.cur_accent);
            }
        }
        // Lines out of sight: said, faintly, at the right of the last row.
        let below = lines.len().saturating_sub(scroll_r + fits.0);
        if scroll_r > 0 || below > 0 {
            let more = match (scroll_r, below) {
                (0, b) => format!("{b} more ↓"),
                (a, 0) => format!("↑ {a} more"),
                (a, b) => format!("↑ {a}  ·  {b} ↓"),
            };
            let mw = self.fonts.small.width(&more);
            self.canvas.text(&mut self.fonts.small, &more, (body.x + body.w - pad - mw) as f32, body.y + body.h - sh - px(4.0), numbers, body);
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
        self.canvas.round_fill(run, px(7.0), if run_lit { mix(accent, 0xFFFFFF, 30) } else { accent }, 255);
        let fg = crate::canvas::on(accent);
        let tx = self.canvas.text(&mut self.fonts.ui, label, (run.x + px(12.0)) as f32, run.y + (bh - uh) / 2, fg, run);
        self.canvas.text(&mut self.fonts.small, keys, (tx + px(10.0)) as f32, run.y + (bh - smh) / 2, mix(accent, fg, 170), run);
        self.canvas.text(&mut self.fonts.small, hint, (r.x + pad) as f32, r.y + r.h - foot + (foot - sh) / 2, c.dim, Rect { w: (run.x - r.x - pad).max(0), ..r });
        let e = &mut self.editors[i];
        e.at = r;
        e.run_at = run;
        e.close_at = close;
        self.damage.push(r);
    }
}
