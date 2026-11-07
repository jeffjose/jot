//! Glyphs, rasterized once and kept: Fira Code for the terminal, bold and
//! regular, with a system font found by fontconfig for any character it
//! does not have. Box-drawing and block characters are drawn from their
//! shapes instead, to fill the cell exactly and join their neighbours.

use std::collections::HashMap;
use std::process::Command;

use fontdue::{Font, FontSettings};

const REGULAR: &[u8] = include_bytes!("../assets/FiraCode-Medium.ttf");
const BOLD: &[u8] = include_bytes!("../assets/FiraCode-Bold.ttf");
const UI: &[u8] = include_bytes!("../assets/Geist-Medium.ttf");

/// A glyph's coverage, placed relative to the top left of its cell.
pub struct Glyph {
    pub left: i32,
    pub top: i32,
    pub width: usize,
    pub height: usize,
    pub coverage: Vec<u8>,
}

pub struct Fonts {
    pub px: f32,
    pub cell_w: i32,
    pub cell_h: i32,
    baseline: i32,
    regular: Font,
    bold: Font,
    /// Fonts found for characters Fira Code lacks, by file.
    fallback: Vec<Font>,
    fallback_files: HashMap<String, usize>,
    /// Which font has each character: None for Fira Code, else a
    /// fallback; a character no font has is drawn with Fira Code's blank.
    which: HashMap<char, Option<usize>>,
    cache: HashMap<(char, bool), Glyph>,
    pub ui: UiFont,
}

fn load(bytes: &[u8]) -> Font {
    Font::from_bytes(bytes, FontSettings::default()).expect("bundled font")
}

impl Fonts {
    pub fn new(px: f32, ui_px: f32) -> Fonts {
        let regular = load(REGULAR);
        let bold = load(BOLD);
        let m = regular.horizontal_line_metrics(px).expect("line metrics");
        let cell_w = regular.metrics('M', px).advance_width.round().max(1.0) as i32;
        let cell_h = (m.ascent - m.descent + m.line_gap).ceil().max(1.0) as i32;
        let baseline = (m.ascent + m.line_gap / 2.0).round() as i32;
        Fonts {
            px,
            cell_w,
            cell_h,
            baseline,
            regular,
            bold,
            fallback: Vec::new(),
            fallback_files: HashMap::new(),
            which: HashMap::new(),
            cache: HashMap::new(),
            ui: UiFont::new(ui_px),
        }
    }

    fn font_for(&mut self, c: char) -> Option<usize> {
        if let Some(&w) = self.which.get(&c) {
            return w;
        }
        let w = if self.regular.lookup_glyph_index(c) != 0 { None } else { self.find_fallback(c) };
        self.which.insert(c, w);
        w
    }

    /// Asks fontconfig for a font with `c`, once per character.
    fn find_fallback(&mut self, c: char) -> Option<usize> {
        let pattern = format!("monospace:charset={:x}:color=false", c as u32);
        let out = Command::new("fc-match").args(["-f", "%{file}", &pattern]).output().ok()?;
        let file = String::from_utf8(out.stdout).ok()?;
        if let Some(&i) = self.fallback_files.get(&file) {
            return (self.fallback[i].lookup_glyph_index(c) != 0).then_some(i);
        }
        let font = Font::from_bytes(std::fs::read(&file).ok()?, FontSettings::default()).ok()?;
        let i = self.fallback.len();
        self.fallback.push(font);
        self.fallback_files.insert(file, i);
        (self.fallback[i].lookup_glyph_index(c) != 0).then_some(i)
    }

    pub fn glyph(&mut self, c: char, bold: bool) -> &Glyph {
        if !self.cache.contains_key(&(c, bold)) {
            let g = match shapes::draw(c, self.cell_w, self.cell_h) {
                Some(g) => g,
                None => self.rasterize(c, bold),
            };
            self.cache.insert((c, bold), g);
        }
        &self.cache[&(c, bold)]
    }

    fn rasterize(&mut self, c: char, bold: bool) -> Glyph {
        let which = self.font_for(c);
        let font = match which {
            Some(i) => &self.fallback[i],
            None if bold => &self.bold,
            None => &self.regular,
        };
        let (m, coverage) = font.rasterize(c, self.px);
        Glyph { left: m.xmin, top: self.baseline - (m.height as i32 + m.ymin), width: m.width, height: m.height, coverage }
    }
}

/// Geist, for jterm's own captions and status line.
pub struct UiFont {
    font: Font,
    pub px: f32,
    pub ascent: i32,
    pub height: i32,
    cache: HashMap<char, (fontdue::Metrics, Vec<u8>)>,
}

impl UiFont {
    fn new(px: f32) -> UiFont {
        let font = load(UI);
        let m = font.horizontal_line_metrics(px).expect("line metrics");
        UiFont { font, px, ascent: m.ascent.round() as i32, height: (m.ascent - m.descent).ceil() as i32, cache: HashMap::new() }
    }

    pub fn glyph(&mut self, c: char) -> &(fontdue::Metrics, Vec<u8>) {
        let (font, px) = (&self.font, self.px);
        self.cache.entry(c).or_insert_with(|| font.rasterize(c, px))
    }

    pub fn width(&mut self, text: &str) -> i32 {
        text.chars().map(|c| self.glyph(c).0.advance_width).sum::<f32>().round() as i32
    }
}

/// Box-drawing (U+2500–257F) and block (U+2580–259F) characters drawn
/// from their shapes.
mod shapes {
    use super::Glyph;

    /// Line weights of the four arms, up, right, down, left: 0 none,
    /// 1 light, 2 heavy, 3 double.
    fn arms(c: char) -> Option<[u8; 4]> {
        Some(match c {
            '─' => [0, 1, 0, 1],
            '━' => [0, 2, 0, 2],
            '│' => [1, 0, 1, 0],
            '┃' => [2, 0, 2, 0],
            '┌' => [0, 1, 1, 0],
            '┍' => [0, 2, 1, 0],
            '┎' => [0, 1, 2, 0],
            '┏' => [0, 2, 2, 0],
            '┐' => [0, 0, 1, 1],
            '┑' => [0, 0, 1, 2],
            '┒' => [0, 0, 2, 1],
            '┓' => [0, 0, 2, 2],
            '└' => [1, 1, 0, 0],
            '┕' => [1, 2, 0, 0],
            '┖' => [2, 1, 0, 0],
            '┗' => [2, 2, 0, 0],
            '┘' => [1, 0, 0, 1],
            '┙' => [1, 0, 0, 2],
            '┚' => [2, 0, 0, 1],
            '┛' => [2, 0, 0, 2],
            '├' => [1, 1, 1, 0],
            '┝' => [1, 2, 1, 0],
            '┠' => [2, 1, 2, 0],
            '┣' => [2, 2, 2, 0],
            '┤' => [1, 0, 1, 1],
            '┥' => [1, 0, 1, 2],
            '┨' => [2, 0, 2, 1],
            '┫' => [2, 0, 2, 2],
            '┬' => [0, 1, 1, 1],
            '┯' => [0, 2, 1, 2],
            '┰' => [0, 1, 2, 1],
            '┳' => [0, 2, 2, 2],
            '┴' => [1, 1, 0, 1],
            '┷' => [1, 2, 0, 2],
            '┸' => [2, 1, 0, 1],
            '┻' => [2, 2, 0, 2],
            '┼' => [1, 1, 1, 1],
            '┿' => [1, 2, 1, 2],
            '╂' => [2, 1, 2, 1],
            '╋' => [2, 2, 2, 2],
            '╴' => [0, 0, 0, 1],
            '╵' => [1, 0, 0, 0],
            '╶' => [0, 1, 0, 0],
            '╷' => [0, 0, 1, 0],
            '╸' => [0, 0, 0, 2],
            '╹' => [2, 0, 0, 0],
            '╺' => [0, 2, 0, 0],
            '╻' => [0, 0, 2, 0],
            '═' => [0, 3, 0, 3],
            '║' => [3, 0, 3, 0],
            '╔' => [0, 3, 3, 0],
            '╗' => [0, 0, 3, 3],
            '╚' => [3, 3, 0, 0],
            '╝' => [3, 0, 0, 3],
            '╠' => [3, 3, 3, 0],
            '╣' => [3, 0, 3, 3],
            '╦' => [0, 3, 3, 3],
            '╩' => [3, 3, 0, 3],
            '╬' => [3, 3, 3, 3],
            _ => return None,
        })
    }

    struct Bits {
        cov: Vec<u8>,
        w: i32,
        h: i32,
    }

    impl Bits {
        fn fill(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, a: u8) {
            for y in y0.max(0)..y1.min(self.h) {
                for x in x0.max(0)..x1.min(self.w) {
                    let p = &mut self.cov[(y * self.w + x) as usize];
                    *p = (*p).max(a);
                }
            }
        }
    }

    pub fn draw(c: char, w: i32, h: i32) -> Option<Glyph> {
        let (wu, hu) = (w as usize, h as usize);
        let mut b = Bits { cov: vec![0u8; wu * hu], w, h };
        let light = (h as f32 / 18.0).round().max(1.0) as i32;
        let heavy = light * 2;
        let (cx, cy) = (w / 2, h / 2);
        if let Some(arms) = arms(c) {
            let t = |weight: u8| if weight == 2 { heavy } else { light };
            // Horizontal arms run to the middle and as far again as the
            // vertical line is thick, so the corners close.
            let vt = t(arms[0].max(arms[2]));
            let ht = t(arms[1].max(arms[3]));
            for (i, &weight) in arms.iter().enumerate() {
                if weight == 0 {
                    continue;
                }
                let th = t(weight);
                let offsets: &[i32] = if weight == 3 { &[-light, light] } else { &[0] };
                for &o in offsets {
                    match i {
                        0 => b.fill(cx - th / 2 + o, 0, cx - th / 2 + o + th, cy + ht / 2 + ht % 2 + if weight == 3 { light } else { 0 }, 255),
                        2 => b.fill(cx - th / 2 + o, cy - ht / 2 - if weight == 3 { light } else { 0 }, cx - th / 2 + o + th, h, 255),
                        1 => b.fill(cx - vt / 2 - if weight == 3 { light } else { 0 }, cy - th / 2 + o, w, cy - th / 2 + o + th, 255),
                        _ => b.fill(0, cy - th / 2 + o, cx + vt / 2 + vt % 2 + if weight == 3 { light } else { 0 }, cy - th / 2 + o + th, 255),
                    }
                }
            }
            return Some(Glyph { left: 0, top: 0, width: wu, height: hu, coverage: b.cov });
        }
        // Rounded corners: a quarter of a circle from the middle of one
        // edge to the middle of the other, smoothed.
        if let Some((sx, sy)) = match c {
            '╭' => Some((1, 1)),
            '╮' => Some((-1, 1)),
            '╯' => Some((-1, -1)),
            '╰' => Some((1, -1)),
            _ => None,
        } {
            let r = (w.min(h) / 2) as f32;
            // The circle's centre, inside the corner the arc bends round.
            let (ox, oy) = (cx as f32 + 0.5 * light as f32 % 2.0 + sx as f32 * r, cy as f32 + sy as f32 * r);
            let half = light as f32 / 2.0;
            for y in 0..h {
                for x in 0..w {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let dx = px - ox;
                    let dy = py - oy;
                    let inside = dx * sx as f32 <= 0.0 && dy * sy as f32 <= 0.0;
                    let a = if inside {
                        let d = ((dx * dx + dy * dy).sqrt() - r).abs();
                        ((half + 0.5 - d).clamp(0.0, 1.0) * 255.0) as u8
                    } else {
                        0
                    };
                    b.cov[y as usize * wu + x as usize] = a;
                }
            }
            // The straight runs beyond the arc to the cell's edges.
            let lx = cx - light / 2;
            let ly = cy - light / 2;
            if sy == 1 {
                b.fill(lx, (oy) as i32, lx + light, h, 255);
            } else {
                b.fill(lx, 0, lx + light, oy.ceil() as i32, 255);
            }
            if sx == 1 {
                b.fill(ox as i32, ly, w, ly + light, 255);
            } else {
                b.fill(0, ly, ox.ceil() as i32, ly + light, 255);
            }
            return Some(Glyph { left: 0, top: 0, width: wu, height: hu, coverage: b.cov });
        }
        let eighth_h = |n: i32| (h * n + 4) / 8;
        let eighth_w = |n: i32| (w * n + 4) / 8;
        let quad = |b: &mut Bits, q: [bool; 4]| {
            // top left, top right, bottom left, bottom right
            if q[0] {
                b.fill(0, 0, cx, cy, 255);
            }
            if q[1] {
                b.fill(cx, 0, w, cy, 255);
            }
            if q[2] {
                b.fill(0, cy, cx, h, 255);
            }
            if q[3] {
                b.fill(cx, cy, w, h, 255);
            }
        };
        match c {
            '▀' => b.fill(0, 0, w, cy, 255),
            '▁'..='▇' | '█' => {
                let n = c as i32 - '▀' as i32;
                b.fill(0, h - eighth_h(n), w, h, 255);
            }
            '▉'..='▏' => {
                let n = 8 - (c as i32 - '█' as i32);
                b.fill(0, 0, eighth_w(n), h, 255);
            }
            '▐' => b.fill(cx, 0, w, h, 255),
            '░' => b.fill(0, 0, w, h, 64),
            '▒' => b.fill(0, 0, w, h, 128),
            '▓' => b.fill(0, 0, w, h, 192),
            '▔' => b.fill(0, 0, w, eighth_h(1), 255),
            '▕' => b.fill(w - eighth_w(1), 0, w, h, 255),
            '▖' => quad(&mut b, [false, false, true, false]),
            '▗' => quad(&mut b, [false, false, false, true]),
            '▘' => quad(&mut b, [true, false, false, false]),
            '▙' => quad(&mut b, [true, false, true, true]),
            '▚' => quad(&mut b, [true, false, false, true]),
            '▛' => quad(&mut b, [true, true, true, false]),
            '▜' => quad(&mut b, [true, true, false, true]),
            '▝' => quad(&mut b, [false, true, false, false]),
            '▞' => quad(&mut b, [false, true, true, false]),
            '▟' => quad(&mut b, [false, true, true, true]),
            _ => return None,
        }
        Some(Glyph { left: 0, top: 0, width: wu, height: hu, coverage: b.cov })
    }
}
