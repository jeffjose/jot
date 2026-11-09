//! The window's pixels, kept between frames so a frame only draws what
//! changed: 0x00RRGGBB, a row after another. Drawing can be kept inside a
//! rectangle, as a page that scrolls is.

use crate::font::{Glyph, UiFont};
use crate::layout::Rect;

pub struct Canvas {
    pub pixels: Vec<u32>,
    pub width: i32,
    pub height: i32,
    /// Nothing is drawn outside this, when set.
    pub clip: Option<Rect>,
}

fn blend(dst: u32, src: u32, a: u32) -> u32 {
    if a >= 255 {
        return src;
    }
    let inv = 255 - a;
    let ch = |shift: u32| ((((src >> shift) & 0xff) * a + ((dst >> shift) & 0xff) * inv + 127) / 255) << shift;
    ch(16) | ch(8) | ch(0)
}

/// `a` taken a share of the way to `b`, 0 to 255.
pub fn mix(a: u32, b: u32, share: u32) -> u32 {
    blend(a, b, share)
}

/// The color for words on `color`: white, or near black on one light
/// enough that white would be lost on it.
pub fn on(color: u32) -> u32 {
    let c = |shift: u32| f64::from(color >> shift & 0xFF);
    if 0.299 * c(16) + 0.587 * c(8) + 0.114 * c(0) > 160.0 {
        0x161616
    } else {
        0xFFFFFF
    }
}

impl Canvas {
    pub fn new() -> Canvas {
        Canvas { pixels: Vec::new(), width: 0, height: 0, clip: None }
    }

    pub fn resize(&mut self, width: i32, height: i32) {
        self.width = width.max(1);
        self.height = height.max(1);
        // The memory kept from one size to the next: a drag goes through
        // many, and a fresh 33 MB at each (at 4K) is a page fault for every
        // 4 KB of it. Nor is it cleared: what it held is drawn over whole by
        // the full frame a new size brings.
        self.pixels.resize((self.width * self.height) as usize, 0);
    }

    /// The part of `r` that may be drawn in: on the canvas, inside the clip.
    fn bounds(&self, r: Rect) -> (i32, i32, i32, i32) {
        let (mut x0, mut y0, mut x1, mut y1) = (r.x.max(0), r.y.max(0), (r.x + r.w).min(self.width), (r.y + r.h).min(self.height));
        if let Some(c) = self.clip {
            x0 = x0.max(c.x);
            y0 = y0.max(c.y);
            x1 = x1.min(c.x + c.w);
            y1 = y1.min(c.y + c.h);
        }
        (x0, y0, x1, y1)
    }

    pub fn fill(&mut self, r: Rect, color: u32) {
        let (x0, y0, x1, y1) = self.bounds(r);
        if x0 >= x1 {
            return;
        }
        for y in y0..y1 {
            let row = (y * self.width) as usize;
            self.pixels[row + x0 as usize..row + x1 as usize].fill(color);
        }
    }

    /// What `r` holds now, kept to be put back with `restore`.
    pub fn save(&self, r: Rect) -> (Rect, Vec<u32>) {
        let (x0, y0, x1, y1) = (r.x.max(0), r.y.max(0), (r.x + r.w).min(self.width), (r.y + r.h).min(self.height));
        let r = Rect { x: x0, y: y0, w: (x1 - x0).max(0), h: (y1 - y0).max(0) };
        let mut px = Vec::with_capacity((r.w * r.h) as usize);
        for y in y0..y1 {
            let row = (y * self.width) as usize;
            px.extend_from_slice(&self.pixels[row + x0 as usize..row + x1 as usize]);
        }
        (r, px)
    }

    /// What `save` kept, put back where it was.
    pub fn restore(&mut self, (r, px): &(Rect, Vec<u32>)) {
        if r.x + r.w > self.width || r.y + r.h > self.height || px.len() != (r.w * r.h) as usize {
            return;
        }
        for y in 0..r.h {
            let row = ((r.y + y) * self.width + r.x) as usize;
            self.pixels[row..row + r.w as usize].copy_from_slice(&px[(y * r.w) as usize..((y + 1) * r.w) as usize]);
        }
    }

    /// `r` filled but for the holes in it (which are filled by what is
    /// drawn there): at 4K the whole canvas is 33 MB written for nothing
    /// where panes paint their own.
    pub fn fill_except(&mut self, r: Rect, holes: &[Rect], color: u32) {
        let (x0, y0, x1, y1) = self.bounds(r);
        let mut cuts: Vec<(i32, i32)> = Vec::with_capacity(holes.len());
        for y in y0..y1 {
            cuts.clear();
            cuts.extend(holes.iter().filter(|h| h.y <= y && y < h.y + h.h).map(|h| (h.x.max(x0), (h.x + h.w).min(x1))));
            cuts.sort_unstable();
            let row = (y * self.width) as usize;
            let mut x = x0;
            for &(a, b) in &cuts {
                if a > x {
                    self.pixels[row + x as usize..row + a as usize].fill(color);
                }
                x = x.max(b);
            }
            if x < x1 {
                self.pixels[row + x as usize..row + x1 as usize].fill(color);
            }
        }
    }

    /// A rectangle with rounded corners, its edges smoothed, laid over
    /// what is there at `alpha` (0 to 255).
    pub fn round_fill(&mut self, r: Rect, radius: i32, color: u32, alpha: u32) {
        self.round_fill_except(r, radius, color, alpha, None);
    }

    /// `round_fill`, but for `hole` (with rounded corners of `hole_radius`
    /// as a panel's): left as it is, as what is drawn there next covers it
    /// (a panel over its shadow), so its pixels are not blended for nothing.
    /// Only the pixels in a corner's curve are worked out one by one; the
    /// rest of each row is one span, filled at once.
    fn round_fill_except(&mut self, r: Rect, radius: i32, color: u32, alpha: u32, hole: Option<(Rect, i32)>) {
        if alpha == 0 {
            return;
        }
        let radius = radius.min(r.w / 2).min(r.h / 2).max(0);
        let rad = radius as f32;
        let (x0, y0, x1, y1) = self.bounds(r);
        // Where the hole covers a row whole: inside it, and clear of its
        // corners' curves.
        let covered = |y: i32| -> Option<(i32, i32)> {
            let (h, hr) = hole?;
            if y < h.y || y >= h.y + h.h {
                return None;
            }
            let inset = if y < h.y + hr || y >= h.y + h.h - hr { hr } else { 0 };
            Some((h.x + inset, h.x + h.w - inset))
        };
        let span = |canvas: &mut Canvas, row: usize, a: i32, b: i32| {
            if a >= b {
                return;
            }
            let s = &mut canvas.pixels[row + a as usize..row + b as usize];
            if alpha >= 255 {
                s.fill(color);
            } else {
                for p in s {
                    *p = blend(*p, color, alpha);
                }
            }
        };
        for y in y0..y1 {
            let row = (y * self.width) as usize;
            let dy = if y < r.y + radius { (r.y + radius) as f32 - (y as f32 + 0.5) } else if y >= r.y + r.h - radius { (y as f32 + 0.5) - (r.y + r.h - radius) as f32 } else { 0.0 };
            // In a corner's rows, the pixels left and right of the straight
            // stretch one by one; else the row is straight end to end.
            let (left, right) = if dy > 0.0 { ((r.x + radius).clamp(x0, x1), (r.x + r.w - radius).clamp(x0, x1)) } else { (x0, x1) };
            for x in (x0..left).chain(right..x1) {
                let dx = if x < r.x + radius { (r.x + radius) as f32 - (x as f32 + 0.5) } else { (x as f32 + 0.5) - (r.x + r.w - radius) as f32 };
                let cover = if dx > 0.0 { (rad + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0) } else { 1.0 };
                let a = (cover * alpha as f32) as u32;
                if a > 0 && !covered(y).is_some_and(|(a0, b0)| x >= a0 && x < b0) {
                    let p = &mut self.pixels[row + x as usize];
                    *p = blend(*p, color, a);
                }
            }
            match covered(y) {
                Some((a0, b0)) => {
                    span(self, row, left, a0.clamp(left, right));
                    span(self, row, b0.clamp(left, right), right);
                }
                None => span(self, row, left, right),
            }
        }
    }

    /// A round dot centred on (cx, cy), `r` across half, its edge smoothed
    /// by how much of each pixel it covers: the same at any size, unlike a
    /// rounded square, which is flat at odd sizes.
    pub fn disc(&mut self, cx: f32, cy: f32, r: f32, color: u32) {
        self.annulus(cx, cy, 0.0, r, color);
    }

    /// A round ring centred on (cx, cy), from radius `inner` out to `outer`,
    /// its edges smoothed.
    pub fn annulus(&mut self, cx: f32, cy: f32, inner: f32, outer: f32, color: u32) {
        let reach = outer.ceil() as i32 + 1;
        let r = Rect { x: cx.floor() as i32 - reach, y: cy.floor() as i32 - reach, w: 2 * reach + 1, h: 2 * reach + 1 };
        let (x0, y0, x1, y1) = self.bounds(r);
        for y in y0..y1 {
            let row = (y * self.width) as usize;
            for x in x0..x1 {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let cover = (outer + 0.5 - d).clamp(0.0, 1.0) * if inner > 0.0 { (d - inner + 0.5).clamp(0.0, 1.0) } else { 1.0 };
                let a = (cover * 255.0) as u32;
                if a > 0 {
                    let p = &mut self.pixels[row + x as usize];
                    *p = blend(*p, color, a);
                }
            }
        }
    }

    /// A rounded outline `width` thick, the inside left as it is.
    pub fn ring(&mut self, r: Rect, radius: i32, width: i32, color: u32) {
        // Drawn row by row as the difference of two rounded shapes.
        let inner = Rect { x: r.x + width, y: r.y + width, w: r.w - 2 * width, h: r.h - 2 * width };
        let (x0, y0, x1, y1) = self.bounds(r);
        let inside = |x: i32, y: i32, q: Rect, rad: i32| -> f32 {
            let rad = rad.min(q.w / 2).min(q.h / 2).max(0);
            if x < q.x || x >= q.x + q.w || y < q.y || y >= q.y + q.h {
                return 0.0;
            }
            let dx = if x < q.x + rad { (q.x + rad) as f32 - (x as f32 + 0.5) } else if x >= q.x + q.w - rad { (x as f32 + 0.5) - (q.x + q.w - rad) as f32 } else { 0.0 };
            let dy = if y < q.y + rad { (q.y + rad) as f32 - (y as f32 + 0.5) } else if y >= q.y + q.h - rad { (y as f32 + 0.5) - (q.y + q.h - rad) as f32 } else { 0.0 };
            if dx > 0.0 && dy > 0.0 { (rad as f32 + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0) } else { 1.0 }
        };
        for y in y0..y1 {
            let row = (y * self.width) as usize;
            for x in x0..x1 {
                let a = (inside(x, y, r, radius) - inside(x, y, inner, radius - width)).clamp(0.0, 1.0);
                if a > 0.0 {
                    let p = &mut self.pixels[row + x as usize];
                    *p = blend(*p, color, (a * 255.0) as u32);
                }
            }
        }
    }

    /// A copy of `r`'s pixels, scaled down to `width` wide.
    pub fn snapshot(&self, r: Rect, width: i32) -> (Vec<u32>, i32, i32) {
        let width = width.min(r.w).max(1);
        let height = (r.h * width / r.w.max(1)).max(1);
        let mut out = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            let sy = (r.y + y * r.h / height).clamp(0, self.height - 1);
            for x in 0..width {
                let sx = (r.x + x * r.w / width).clamp(0, self.width - 1);
                out.push(self.pixels[(sy * self.width + sx) as usize]);
            }
        }
        (out, width, height)
    }

    /// Pixels laid over the canvas at `(x, y)`, `alpha` of the way.
    pub fn blit(&mut self, pixels: &[u32], w: i32, h: i32, x: i32, y: i32, alpha: u32) {
        let (x0, y0, x1, y1) = self.bounds(Rect { x, y, w, h });
        for py in y0..y1 {
            let row = (py * self.width) as usize;
            for px in x0..x1 {
                let src = pixels[((py - y) * w + (px - x)) as usize];
                let p = &mut self.pixels[row + px as usize];
                *p = blend(*p, src, alpha);
            }
        }
    }

    /// Part of a picture (`src` x, y, w, h in its pixels) scaled into `dest`,
    /// each pixel laid over by its own alpha, 0xAARRGGBB, kept in `clip`.
    pub fn picture(&mut self, pixels: &[u32], pw: u32, src: (u32, u32, u32, u32), dest: Rect, clip: Rect) {
        let (x0, y0, x1, y1) = self.bounds(Rect {
            x: dest.x.max(clip.x),
            y: dest.y.max(clip.y),
            w: (dest.x + dest.w).min(clip.x + clip.w) - dest.x.max(clip.x),
            h: (dest.y + dest.h).min(clip.y + clip.h) - dest.y.max(clip.y),
        });
        let (sw, sh) = (src.2.max(1) as i64, src.3.max(1) as i64);
        for y in y0..y1 {
            let sy = src.1 as i64 + (y - dest.y) as i64 * sh / dest.h.max(1) as i64;
            let row = (y * self.width) as usize;
            for x in x0..x1 {
                let sx = src.0 as i64 + (x - dest.x) as i64 * sw / dest.w.max(1) as i64;
                let Some(&s) = pixels.get((sy * pw as i64 + sx) as usize) else { continue };
                let a = s >> 24;
                if a > 0 {
                    let p = &mut self.pixels[row + x as usize];
                    *p = blend(*p, s & 0xFFFFFF, a);
                }
            }
        }
    }

    /// A floating panel, as jog draws its menus: a soft shadow, a thin
    /// edge, and a faint light along the top.
    pub fn panel(&mut self, r: Rect, radius: i32, fill: u32, edge: u32) {
        let spread = (radius * 3 / 2).clamp(6, 16);
        for i in (1..=spread).step_by(2) {
            let s = Rect { x: r.x - i, y: r.y - i + spread / 3, w: r.w + 2 * i, h: r.h + 2 * i };
            // Not under the panel itself, which covers it whole.
            self.round_fill_except(s, radius + i, 0x000000, (28 * (spread - i + 1) / spread) as u32, Some((r, radius)));
        }
        self.round_fill(r, radius, edge, 255);
        self.round_fill(Rect { x: r.x + 1, y: r.y + 1, w: r.w - 2, h: r.h - 2 }, radius - 1, fill, 255);
        self.round_fill(Rect { x: r.x + radius, y: r.y + 1, w: r.w - 2 * radius, h: 1 }, 0, 0xFFFFFF, 18);
    }

    /// A glyph's coverage laid over the canvas in `color`, its cell's top
    /// left at `(x, y)`, kept inside `clip`.
    pub fn glyph(&mut self, g: &Glyph, x: i32, y: i32, color: u32, clip: Rect) {
        if let Some(px) = &g.color {
            // An emoji: its own colors, laid over by their alpha.
            let (x0, y0, x1, y1) = self.bounds(clip);
            let (gx, gy) = (x + g.left, y + g.top);
            for py in gy.max(y0)..(gy + g.height as i32).min(y1) {
                for qx in gx.max(x0)..(gx + g.width as i32).min(x1) {
                    let s = px[((py - gy) * g.width as i32 + (qx - gx)) as usize];
                    let a = s >> 24;
                    if a > 0 {
                        let p = &mut self.pixels[(py * self.width + qx) as usize];
                        *p = blend(*p, s & 0xFFFFFF, a);
                    }
                }
            }
            return;
        }
        self.coverage(&g.coverage, g.width, g.height, x + g.left, y + g.top, color, clip, g.lcd);
    }

    /// Text's coverage laid on in `color`: one coverage a pixel, or with
    /// `lcd` three, one for each of its red, green and blue stripes.
    #[allow(clippy::too_many_arguments)]
    fn coverage(&mut self, cov: &[u8], w: usize, h: usize, x: i32, y: i32, color: u32, clip: Rect, lcd: bool) {
        let (cx0, cy0, cx1, cy1) = self.bounds(clip);
        // Blended as sRGB, as browsers and editors blend text; light text
        // has its edges filled a little more, as Skia does, so that on a
        // dark ground it is not thinner than the same text dark on light.
        let (lift, text_l) = (text_lift(), luminance(color));
        let mut ground = (u32::MAX, false);
        let n = if lcd { 3 } else { 1 };
        for gy in 0..h as i32 {
            let py = y + gy;
            if py < cy0 || py >= cy1 {
                continue;
            }
            let row = (py * self.width) as usize;
            for gx in 0..w as i32 {
                let px = x + gx;
                if px < cx0 || px >= cx1 {
                    continue;
                }
                let at = (gy as usize * w + gx as usize) * n;
                let a = &cov[at..at + n];
                if a.iter().all(|&v| v == 0) {
                    continue;
                }
                let p = &mut self.pixels[row + px as usize];
                if a.iter().all(|&v| v == 255) {
                    *p = color;
                    continue;
                }
                // Whether the text is lighter than its ground, kept while
                // the ground stays the same color.
                if ground.0 != *p {
                    ground = (*p, text_l > luminance(*p));
                }
                let take = |v: u8| if ground.1 { lift[v as usize] as u32 } else { v as u32 };
                *p = if lcd {
                    let ch = |shift: u32, v: u8| {
                        let (s, d, k) = ((color >> shift) & 0xff, (*p >> shift) & 0xff, take(v));
                        ((s * k + d * (255 - k) + 127) / 255) << shift
                    };
                    ch(16, a[0]) | ch(8, a[1]) | ch(0, a[2])
                } else {
                    blend(*p, color, take(a[0]))
                };
            }
        }
    }


    /// Text in an interface font, its top at `y`. Returns where it ends.
    pub fn text(&mut self, font: &mut UiFont, text: &str, mut x: f32, y: i32, color: u32, clip: Rect) -> i32 {
        let (ascent, lcd) = (font.ascent, font.lcd.is_some());
        for c in text.chars() {
            let (m, cov) = font.glyph(c);
            let (gx, gy) = (x.round() as i32 + m.xmin, y + ascent - (m.height as i32 + m.ymin));
            let (w, h, adv) = (m.width, m.height, m.advance_width);
            let cov = cov.clone();
            self.coverage(&cov, w, h, gx, gy, color, clip, lcd);
            x += adv;
        }
        x.round() as i32
    }

    /// Text cut to `room` pixels wide, with … where it was cut.
    pub fn text_fit(&mut self, font: &mut UiFont, text: &str, x: i32, y: i32, room: i32, color: u32, clip: Rect) -> i32 {
        if font.width(text) <= room {
            return self.text(font, text, x as f32, y, color, clip);
        }
        let mut cut: String = text.to_string();
        while !cut.is_empty() && font.width(&format!("{cut}…")) > room {
            cut.pop();
        }
        self.text(font, &format!("{cut}…"), x as f32, y, color, clip)
    }
}

/// sRGB to linear light, 0 to 4095, and back: text is laid over its
/// ground in linear light, as browsers and VS Code draw it, so light text
/// on a dark ground keeps the weight its font gives it.
fn tables() -> &'static ([u16; 256], [u8; 4096]) {
    static T: std::sync::OnceLock<([u16; 256], [u8; 4096])> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let to_lin = |c: f64| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        let to_srgb = |l: f64| if l <= 0.0031308 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
        let mut lin = [0u16; 256];
        for (i, v) in lin.iter_mut().enumerate() {
            *v = (to_lin(i as f64 / 255.0) * 4095.0).round() as u16;
        }
        let mut srgb = [0u8; 4096];
        for (i, v) in srgb.iter_mut().enumerate() {
            *v = (to_srgb(i as f64 / 4095.0) * 255.0).round() as u8;
        }
        (lin, srgb)
    })
}


/// How light a color is to the eye, 0 to 1, as WCAG reckons it.
fn luminance(c: u32) -> f32 {
    let (lin, _) = tables();
    let l = |shift: u32| lin[(c >> shift & 0xff) as usize] as f32 / 4095.0;
    0.2126 * l(16) + 0.7152 * l(8) + 0.0722 * l(0)
}

/// The contrast between two colors, 1 to 21.
pub fn contrast(a: u32, b: u32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `fg`, made darker or lighter, away from `bg`, until it has at least
/// `min` contrast with it, as VS Code's terminal does (its 4.5): bright
/// yellow on white is still read.
pub fn readable(fg: u32, bg: u32, min: f32) -> u32 {
    if min <= 1.0 || contrast(fg, bg) >= min {
        return fg;
    }
    let toward = if luminance(bg) > 0.5 { 0x000000 } else { 0xFFFFFF };
    let mut share = 16;
    while share < 256 {
        let c = blend(fg, toward, share);
        if contrast(c, bg) >= min {
            return c;
        }
        share += 16;
    }
    toward
}

/// Coverage for light text: partly covered pixels taken further on, as
/// a gamma of TEXT_LIFT would.
fn text_lift() -> &'static [u8; 256] {
    static T: std::sync::OnceLock<[u8; 256]> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let g = std::env::var("JOT_TEXT_LIFT").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(TEXT_LIFT);
        std::array::from_fn(|i| ((i as f64 / 255.0).powf(1.0 / g) * 255.0).round() as u8)
    })
}

const TEXT_LIFT: f64 = 1.3;

/// Icons jot carries: Solar's, in its line style at a heavier line, as jog
/// draws them, made into a font by `tools/solar_font.py`. Each is the
/// letter that font has it at.
pub mod icons {
    pub const FOCUS: char = '\u{e219}';
    pub const UNFOCUS: char = '\u{e21f}';
    pub const RESTART: char = '\u{e14a}';
    pub const PLAY: char = '\u{e13f}';
    pub const CLOSE: char = '\u{e1b1}';
    pub const MINUS: char = '\u{e2a4}';
    pub const CHEVRON_DOWN: char = '\u{e2a5}';
    pub const ARROW_RIGHT: char = '\u{e04d}';
    pub const SIDEBAR: char = '\u{e300}';
    pub const PLUS: char = '\u{e301}';
    pub const MONITOR: char = '\u{e302}';
    pub const SERVER: char = '\u{e303}';
    #[allow(dead_code)]
    pub const CHEVRON_RIGHT: char = '\u{e304}';
    pub const SETTINGS: char = '\u{e305}';
    #[allow(dead_code)]
    pub const LAYERS: char = '\u{e306}';
    pub const CHEVRON_UP: char = '\u{e308}';
    /// The window's maximize and restore, in jot's own title bar.
    pub const WIN_MAX: char = '\u{e309}';
    pub const WIN_RESTORE: char = '\u{e30a}';
    /// A toast's mark: done, something to know, something wrong.
    pub const CHECK: char = '\u{e30b}';
    pub const INFO: char = '\u{e30c}';
    pub const DANGER: char = '\u{e30d}';
    /// Projects' marks, one picked by the project's name (see app::projects).
    pub const PROJECT: [char; 16] = ['\u{e310}', '\u{e311}', '\u{e312}', '\u{e313}', '\u{e314}', '\u{e315}', '\u{e316}', '\u{e317}', '\u{e318}', '\u{e319}', '\u{e31a}', '\u{e31b}', '\u{e31c}', '\u{e31d}', '\u{e31e}', '\u{e31f}'];
    /// Workspaces' marks on the foot bar, one picked by the workspace's
    /// name: Solar's bold, plain shapes that hold at 13 pixels.
    pub const WORKSPACE: [char; 16] = ['\u{e320}', '\u{e321}', '\u{e322}', '\u{e323}', '\u{e324}', '\u{e325}', '\u{e326}', '\u{e327}', '\u{e328}', '\u{e329}', '\u{e32a}', '\u{e32b}', '\u{e32c}', '\u{e32d}', '\u{e32e}', '\u{e32f}'];
}

/// An icon's mask at `size` pixels square: its letter drawn to fill the
/// square, as Solar's 24 fill theirs, kept once made.
fn icon_mask(icon: char, size: usize) -> std::rc::Rc<Vec<u8>> {
    thread_local! {
        static FONT: Option<fontdue::Font> = fontdue::Font::from_bytes(&include_bytes!("../assets/solar.ttf")[..], fontdue::FontSettings::default()).ok();
        static MADE: std::cell::RefCell<std::collections::HashMap<(char, usize), std::rc::Rc<Vec<u8>>>> = Default::default();
    }
    if let Some(m) = MADE.with(|m| m.borrow().get(&(icon, size)).cloned()) {
        return m;
    }
    let mut out = vec![0u8; size * size];
    FONT.with(|font| {
        let Some(font) = font else { return };
        // The em is the square, its baseline at the bottom.
        let (m, bits) = font.rasterize(icon, size as f32);
        let top = size as i32 - m.height as i32 - m.ymin;
        for gy in 0..m.height {
            for gx in 0..m.width {
                let (x, y) = (m.xmin + gx as i32, top + gy as i32);
                if x >= 0 && y >= 0 && (x as usize) < size && (y as usize) < size {
                    out[y as usize * size + x as usize] = bits[gy * m.width + gx];
                }
            }
        }
    });
    let m = std::rc::Rc::new(out);
    MADE.with(|made| made.borrow_mut().insert((icon, size), m.clone()));
    m
}

impl Canvas {
    /// One of jot's icons, `size` pixels square, its top left at x, y,
    /// in `color`.
    #[allow(clippy::too_many_arguments)]
    pub fn icon(&mut self, icon: char, size: i32, x: i32, y: i32, color: u32, clip: crate::layout::Rect) {
        let size = size.max(1) as usize;
        let mask = icon_mask(icon, size);
        self.coverage(&mask, size, size, x, y, color, clip, false);
    }

    /// An icon `size` pixels square in the middle of a button.
    pub fn icon_in(&mut self, icon: char, button: crate::layout::Rect, size: i32, color: u32) {
        self.icon(icon, size, button.x + (button.w - size) / 2, button.y + (button.h - size) / 2, color, button);
    }
}
