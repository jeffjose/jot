//! The window's pixels, kept between frames so a frame only draws what
//! changed: 0x00RRGGBB, a row after another.

use crate::font::{Glyph, UiFont};
use crate::layout::Rect;

pub struct Canvas {
    pub pixels: Vec<u32>,
    pub width: i32,
    pub height: i32,
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

impl Canvas {
    pub fn new() -> Canvas {
        Canvas { pixels: Vec::new(), width: 0, height: 0 }
    }

    pub fn resize(&mut self, width: i32, height: i32) {
        self.width = width.max(1);
        self.height = height.max(1);
        self.pixels = vec![0; (self.width * self.height) as usize];
    }

    pub fn fill(&mut self, r: Rect, color: u32) {
        let (x0, x1) = (r.x.clamp(0, self.width), (r.x + r.w).clamp(0, self.width));
        let (y0, y1) = (r.y.clamp(0, self.height), (r.y + r.h).clamp(0, self.height));
        if x0 >= x1 {
            return;
        }
        for y in y0..y1 {
            let row = (y * self.width) as usize;
            self.pixels[row + x0 as usize..row + x1 as usize].fill(color);
        }
    }

    /// A glyph's coverage laid over the canvas in `color`, its cell's top
    /// left at `(x, y)`, kept inside `clip`.
    pub fn glyph(&mut self, g: &Glyph, x: i32, y: i32, color: u32, clip: Rect) {
        self.coverage(&g.coverage, g.width, g.height, x + g.left, y + g.top, color, clip);
    }

    #[allow(clippy::too_many_arguments)]
    fn coverage(&mut self, cov: &[u8], w: usize, h: usize, x: i32, y: i32, color: u32, clip: Rect) {
        let cx0 = clip.x.max(0);
        let cy0 = clip.y.max(0);
        let cx1 = (clip.x + clip.w).min(self.width);
        let cy1 = (clip.y + clip.h).min(self.height);
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
                let a = cov[(gy * w as i32 + gx) as usize] as u32;
                if a > 0 {
                    let p = &mut self.pixels[row + px as usize];
                    *p = blend(*p, color, a);
                }
            }
        }
    }

    /// Text in the interface's font, its baseline `ascent` below `y`.
    /// Returns where it ends.
    pub fn text(&mut self, font: &mut UiFont, text: &str, mut x: f32, y: i32, color: u32, clip: Rect) -> i32 {
        let ascent = font.ascent;
        for c in text.chars() {
            let (m, cov) = font.glyph(c);
            let (gx, gy) = (x.round() as i32 + m.xmin, y + ascent - (m.height as i32 + m.ymin));
            let (w, h, adv) = (m.width, m.height, m.advance_width);
            let cov = cov.clone();
            self.coverage(&cov, w, h, gx, gy, color, clip);
            x += adv;
        }
        x.round() as i32
    }
}
