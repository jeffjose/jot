//! Glyphs, rasterized once and kept. The terminal's font is Fira Code
//! unless another is chosen, bold and regular, with a system font found by
//! fontconfig for any character it does not have; the interface's is
//! Geist unless another is. Box-drawing and block characters are drawn
//! from their shapes instead, to fill the cell exactly and join their
//! neighbours.

use std::collections::HashMap;
use std::process::Command;

use fontdue::{Font, FontSettings};
use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::Format;

const FIRA: &[u8] = include_bytes!("../assets/FiraCode-Medium.ttf");
const FIRA_BOLD: &[u8] = include_bytes!("../assets/FiraCode-Bold.ttf");
const GEIST: &[u8] = include_bytes!("../assets/Geist-Medium.ttf");

/// The fonts jterm has with it, used by these names whatever is installed.
pub const TERMINAL_DEFAULT: &str = "Fira Code";
pub const INTERFACE_DEFAULT: &str = "Geist";

/// Which fonts, at what sizes in points.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub family: String,
    pub size: f32,
    pub ui_family: String,
    pub ui_size: f32,
    /// Joined characters, as Fira Code draws => and !=.
    pub ligatures: bool,
    /// Bold text in the font's bold; else in its usual weight, as VS Code
    /// draws it with a family that has no bold, like Fira Code Medium.
    pub bold: bool,
    /// The height of a line, as a share of the font's own.
    pub line_height: f32,
}

/// A glyph's coverage, placed relative to the top left of its cell.
pub struct Glyph {
    pub left: i32,
    pub top: i32,
    pub width: usize,
    pub height: usize,
    pub coverage: Vec<u8>,
    /// For an emoji, its own colors, alpha in the top byte; then the
    /// coverage is not used.
    pub color: Option<Vec<u32>>,
}

pub struct Fonts {
    pub px: f32,
    pub cell_w: i32,
    pub cell_h: i32,
    baseline: i32,
    regular: Face,
    bold: Face,
    /// Fonts found for characters the terminal's font lacks, by file.
    fallback: Vec<Face>,
    fallback_files: HashMap<String, usize>,
    /// Which font has each character: None for the terminal's own, else
    /// a fallback; a character no font has is drawn with its blank.
    which: HashMap<char, Option<usize>>,
    /// Glyphs by character and style (bold 1, italic 2), and by glyph
    /// number for the ones ligatures make.
    cache: HashMap<(char, u8), Glyph>,
    by_id: HashMap<(u16, u8), Glyph>,
    /// Runs of text shaped, by text and boldness: a glyph a character.
    shaped: [HashMap<String, Option<Vec<u16>>>; 2],
    pub ligatures: bool,
    /// Rasterizes the terminal's glyphs, hinted: outlines snapped to the
    /// pixel grid up and down, as FreeType's light hinting does.
    scaler: ScaleContext,
    /// The interface: its text, headings, and the smaller words under
    /// a setting.
    pub ui: UiFont,
    pub heading: UiFont,
    pub small: UiFont,
}

/// The file fontconfig has for `family` in a weight, if it has that family
/// at all: fc-match answers every question with something.
fn find(family: &str, weight: &str) -> Option<Face> {
    let pattern = format!("{family}:weight={weight}");
    let out = Command::new("fc-match").args(["-f", "%{family}|%{file}|%{index}", &pattern]).output().ok()?;
    let out = String::from_utf8(out.stdout).ok()?;
    let mut parts = out.rsplitn(3, '|');
    let index: u32 = parts.next()?.trim().parse().unwrap_or(0);
    let file = parts.next()?;
    let families = parts.next()?;
    if !families.split(',').any(|f| f.trim().eq_ignore_ascii_case(family.trim())) {
        return None;
    }
    Face::new(std::fs::read(file).ok()?, index)
}

/// A font with its bytes kept, for shaping its ligatures.
pub struct Face {
    pub font: Font,
    bytes: Vec<u8>,
    index: u32,
}

impl Face {
    fn new(bytes: Vec<u8>, index: u32) -> Option<Face> {
        let font = Font::from_bytes(bytes.as_slice(), FontSettings { collection_index: index, ..FontSettings::default() }).ok()?;
        Some(Face { font, bytes, index })
    }

    /// A glyph by number, hinted, as coverage: where its box starts right
    /// of the pen and above the baseline.
    fn raster(&self, ctx: &mut ScaleContext, id: u16, px: f32) -> Raster {
        let image = swash::FontRef::from_index(&self.bytes, self.index as usize).and_then(|font| {
            let mut scaler = ctx.builder(font).size(px).hint(true).build();
            Render::new(&[Source::Outline]).format(Format::Alpha).render(&mut scaler, id)
        });
        match image {
            Some(img) => Raster { left: img.placement.left, top: img.placement.top, width: img.placement.width as usize, height: img.placement.height as usize, coverage: img.data },
            None => Raster { left: 0, top: 0, width: 0, height: 0, coverage: Vec::new() },
        }
    }
}

/// A glyph's coverage as rasterized, before it is placed in a cell.
struct Raster {
    left: i32,
    /// From the baseline up to the top of its box.
    top: i32,
    width: usize,
    height: usize,
    coverage: Vec<u8>,
}

/// The terminal's font, regular and bold.
fn terminal_font(family: &str, with_bold: bool) -> (Face, Face) {
    if !with_bold {
        let (regular, _) = terminal_font(family, true);
        let again = Face::new(regular.bytes.clone(), regular.index).unwrap();
        return (regular, again);
    }
    if !family.eq_ignore_ascii_case(TERMINAL_DEFAULT) {
        if let Some(regular) = find(family, "medium").or_else(|| find(family, "regular")) {
            let bold = find(family, "bold").unwrap_or_else(|| find(family, "regular").unwrap_or_else(|| Face::new(FIRA.to_vec(), 0).unwrap()));
            return (regular, bold);
        }
    }
    (Face::new(FIRA.to_vec(), 0).unwrap(), Face::new(FIRA_BOLD.to_vec(), 0).unwrap())
}

fn interface_font(family: &str) -> Face {
    if !family.eq_ignore_ascii_case(INTERFACE_DEFAULT) {
        if let Some(f) = find(family, "medium").or_else(|| find(family, "regular")) {
            return f;
        }
    }
    Face::new(GEIST.to_vec(), 0).unwrap()
}

/// The font families installed, by name, each once, A to Z: only those
/// whose letters are all one width with `mono`. The fonts jterm has
/// with it are among them.
pub fn families(mono: bool) -> Vec<String> {
    let mut args = vec!["-f", "%{family[0]}\n"];
    if mono {
        args.push(":spacing=100");
    }
    let out = Command::new("fc-list").args(&args).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let mut names: Vec<String> = out.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect();
    names.push(if mono { TERMINAL_DEFAULT } else { INTERFACE_DEFAULT }.to_string());
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    names
}

impl Fonts {
    pub fn new(choice: &Choice, scale: f32) -> Fonts {
        let px = choice.size * scale;
        let ui_px = choice.ui_size * scale;
        let (regular, bold) = terminal_font(&choice.family, choice.bold);
        let m = regular.font.horizontal_line_metrics(px).expect("line metrics");
        let cell_w = regular.font.metrics('M', px).advance_width.round().max(1.0) as i32;
        let natural = m.ascent - m.descent + m.line_gap;
        let cell_h = (natural * choice.line_height.clamp(0.8, 2.0)).round().max(1.0) as i32;
        // The room a taller line adds is shared above and below.
        let baseline = (m.ascent + m.line_gap / 2.0 + (cell_h as f32 - natural) / 2.0).round() as i32;
        let ui = interface_font(&choice.ui_family);
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
            by_id: HashMap::new(),
            shaped: [HashMap::new(), HashMap::new()],
            ligatures: choice.ligatures,
            scaler: ScaleContext::new(),
            heading: UiFont::new(&ui, ui_px * 1.3),
            small: UiFont::new(&ui, ui_px * 0.88),
            ui: UiFont::new(&ui, ui_px),
        }
    }

    fn font_for(&mut self, c: char) -> Option<usize> {
        if let Some(&w) = self.which.get(&c) {
            return w;
        }
        let w = if self.regular.font.lookup_glyph_index(c) != 0 { None } else { self.find_fallback(c) };
        self.which.insert(c, w);
        w
    }

    /// Asks fontconfig for a font with `c`, once per character.
    fn find_fallback(&mut self, c: char) -> Option<usize> {
        let pattern = format!("monospace:charset={:x}:color=false", c as u32);
        let out = Command::new("fc-match").args(["-f", "%{file}", &pattern]).output().ok()?;
        let file = String::from_utf8(out.stdout).ok()?;
        if let Some(&i) = self.fallback_files.get(&file) {
            return (self.fallback[i].font.lookup_glyph_index(c) != 0).then_some(i);
        }
        let font = Face::new(std::fs::read(&file).ok()?, 0)?;
        let i = self.fallback.len();
        self.fallback.push(font);
        self.fallback_files.insert(file, i);
        (self.fallback[i].font.lookup_glyph_index(c) != 0).then_some(i)
    }

    /// A character's glyph in a style: 1 bold, 2 italic, 4 an emoji in one
    /// cell rather than two.
    /// An emoji in color from Twemoji, two cells wide and as tall as a
    /// cell, in their middle; None for a character Twemoji has no
    /// colors for.
    fn emoji(&self, c: char, narrow: bool) -> Option<Glyph> {
        if !emoji_by_default(c) {
            return None;
        }
        let cells = if narrow { 1 } else { 2 };
        let side = self.cell_h.min(cells * self.cell_w) as u32;
        let (w, h, pixels) = crate::emoji::twemoji(&c.to_string(), side)?;
        Some(Glyph { left: (cells * self.cell_w - w) / 2, top: (self.cell_h - h) / 2, width: w as usize, height: h as usize, coverage: Vec::new(), color: Some(pixels) })
    }

    pub fn glyph(&mut self, c: char, style: u8) -> &Glyph {
        if !self.cache.contains_key(&(c, style)) {
            let g = match shapes::draw(c, self.cell_w, self.cell_h).or_else(|| self.emoji(c, style & 4 != 0)) {
                Some(g) => g,
                None => {
                    let which = self.font_for(c);
                    let face = match which {
                        Some(i) => &self.fallback[i],
                        None if style & 1 != 0 => &self.bold,
                        None => &self.regular,
                    };
                    let r = face.raster(&mut self.scaler, face.font.lookup_glyph_index(c), self.px);
                    self.place(r, style)
                }
            };
            self.cache.insert((c, style), g);
        }
        &self.cache[&(c, style)]
    }

    /// A glyph by its number in the terminal's font, as a ligature makes.
    pub fn glyph_id(&mut self, id: u16, style: u8) -> &Glyph {
        if !self.by_id.contains_key(&(id, style)) {
            let face = if style & 1 != 0 { &self.bold } else { &self.regular };
            let r = face.raster(&mut self.scaler, id, self.px);
            let g = self.place(r, style);
            self.by_id.insert((id, style), g);
        }
        &self.by_id[&(id, style)]
    }

    /// A glyph placed in its cell; slanted when italic, there being no
    /// italic face: each row shifted right the higher it is.
    fn place(&self, r: Raster, style: u8) -> Glyph {
        let top = self.baseline - r.top;
        if style & 2 == 0 || r.width == 0 {
            return Glyph { left: r.left, top, width: r.width, height: r.height, coverage: r.coverage, color: None };
        }
        let slant = 0.2;
        let extra = (r.height as f32 * slant).ceil() as usize;
        let w = r.width + extra;
        let mut out = vec![0u8; w * r.height];
        // The shift is measured from the baseline, so the glyph leans
        // about where it stands.
        let base = r.top as f32;
        for y in 0..r.height {
            let shift = ((base - y as f32) * slant).max(0.0);
            let (whole, frac) = (shift.floor() as usize, shift.fract());
            for x in 0..r.width {
                let v = r.coverage[y * r.width + x] as f32;
                let at = x + whole;
                if at < w {
                    out[y * w + at] = (out[y * w + at] as f32 + v * (1.0 - frac)).min(255.0) as u8;
                }
                if at + 1 < w {
                    out[y * w + at + 1] = (out[y * w + at + 1] as f32 + v * frac).min(255.0) as u8;
                }
            }
        }
        Glyph { left: r.left, top, width: w, height: r.height, coverage: out, color: None }
    }

    /// The glyph numbers for a run of text as the font's ligatures draw
    /// it, a glyph a character, or None when they do not line up one to
    /// one or the font changes nothing.
    pub fn shape(&mut self, text: &str, bold: bool) -> Option<Vec<u16>> {
        if !self.ligatures {
            return None;
        }
        if let Some(r) = self.shaped[bold as usize].get(text) {
            return r.clone();
        }
        let face = if bold { &self.bold } else { &self.regular };
        let result = (|| {
            let rb = rustybuzz::Face::from_slice(&face.bytes, face.index)?;
            let mut buf = rustybuzz::UnicodeBuffer::new();
            buf.push_str(text);
            let out = rustybuzz::shape(&rb, &[], buf);
            let ids: Vec<u16> = out.glyph_infos().iter().map(|g| g.glyph_id as u16).collect();
            let plain: Vec<u16> = text.chars().map(|c| face.font.lookup_glyph_index(c)).collect();
            (ids.len() == plain.len() && ids != plain).then_some(ids)
        })();
        if self.shaped[bold as usize].len() > 4096 {
            self.shaped[bold as usize].clear();
        }
        self.shaped[bold as usize].insert(text.to_string(), result.clone());
        result
    }

    /// The glyph number a character has without ligatures.
    pub fn plain_id(&self, c: char, bold: bool) -> u16 {
        if bold { self.bold.font.lookup_glyph_index(c) } else { self.regular.font.lookup_glyph_index(c) }
    }
}

/// An interface font at one size, for jterm's own words.
pub struct UiFont {
    face: Face,
    scaler: ScaleContext,
    pub px: f32,
    pub ascent: i32,
    pub height: i32,
    cache: HashMap<char, (fontdue::Metrics, Vec<u8>)>,
}

impl UiFont {
    fn new(face: &Face, px: f32) -> UiFont {
        let face = Face::new(face.bytes.clone(), face.index).expect("interface font");
        let m = face.font.horizontal_line_metrics(px).expect("line metrics");
        // The ascent on a whole pixel, as hinting puts the tops of letters.
        UiFont { face, scaler: ScaleContext::new(), px, ascent: m.ascent.round() as i32, height: (m.ascent - m.descent).ceil() as i32, cache: HashMap::new() }
    }

    /// A character's coverage, hinted as the terminal's are, with the
    /// font's own advance so words keep their width.
    pub fn glyph(&mut self, c: char) -> &(fontdue::Metrics, Vec<u8>) {
        if !self.cache.contains_key(&c) {
            let mut m = self.face.font.metrics(c, self.px);
            let r = self.face.raster(&mut self.scaler, self.face.font.lookup_glyph_index(c), self.px);
            m.xmin = r.left;
            m.width = r.width;
            m.height = r.height;
            m.ymin = r.top - r.height as i32;
            self.cache.insert(c, (m, r.coverage));
        }
        &self.cache[&c]
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
            return Some(Glyph { left: 0, top: 0, width: wu, height: hu, coverage: b.cov, color: None });
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
            return Some(Glyph { left: 0, top: 0, width: wu, height: hu, coverage: b.cov, color: None });
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
        Some(Glyph { left: 0, top: 0, width: wu, height: hu, coverage: b.cov, color: None })
    }
}


/// Whether a character is drawn as a color emoji when nothing says how:
/// Unicode's Emoji_Presentation. Below U+1F000 most symbols are text by
/// default (✓ ✔ ★ ⌘ ⏎ ☐), drawn in the text's color as other terminals
/// and editors draw them; only these few are pictures.
fn emoji_by_default(c: char) -> bool {
    let u = c as u32;
    if u >= 0x1F000 {
        return true;
    }
    matches!(u,
        0x231A..=0x231B | 0x23E9..=0x23EC | 0x23F0 | 0x23F3 | 0x25FD..=0x25FE | 0x2614..=0x2615 | 0x2648..=0x2653 | 0x267F | 0x2693 | 0x26A1
        | 0x26AA..=0x26AB | 0x26BD..=0x26BE | 0x26C4..=0x26C5 | 0x26CE | 0x26D4 | 0x26EA | 0x26F2..=0x26F3 | 0x26F5 | 0x26FA | 0x26FD
        | 0x2705 | 0x270A..=0x270B | 0x2728 | 0x274C | 0x274E | 0x2753..=0x2755 | 0x2757 | 0x2795..=0x2797 | 0x27B0 | 0x27BF
        | 0x2B1B..=0x2B1C | 0x2B50 | 0x2B55)
}
