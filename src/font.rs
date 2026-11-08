//! Glyphs, rasterized once and kept. The terminal's font is Fira Code
//! unless another is chosen, bold and regular, with a system font found by
//! fontconfig for any character it does not have; the interface's is
//! Geist unless another is. Box-drawing and block characters are drawn
//! from their shapes instead, to fill the cell exactly and join their
//! neighbours.

use std::collections::HashMap;
use std::process::Command;
use std::rc::Rc;

use fontdue::{Font, FontSettings};
use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::Format;

const FIRA: &[u8] = include_bytes!("../assets/FiraCode-Medium.ttf");
const FIRA_BOLD: &[u8] = include_bytes!("../assets/FiraCode-Bold.ttf");
const GEIST: &[u8] = include_bytes!("../assets/Geist-Medium.ttf");

/// The fonts jot has with it, used by these names whatever is installed.
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
    /// Subpixel text, and the order of the screen's stripes; None for gray.
    pub lcd: Option<Lcd>,
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
    /// Three coverages a pixel, red green blue, for subpixel text.
    pub lcd: bool,
}

pub struct Fonts {
    pub px: f32,
    pub cell_w: i32,
    pub cell_h: i32,
    baseline: i32,
    regular: Rc<Face>,
    bold: Rc<Face>,
    /// Fonts found for characters the terminal's font lacks, by file.
    fallback: Vec<Rc<Face>>,
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
    lcd: Option<Lcd>,
    /// The interface: its text, headings, and the smaller words under
    /// a setting.
    pub ui: UiFont,
    pub heading: UiFont,
    pub small: UiFont,
    /// The sidebar's: names a little over the interface's size, details a
    /// little under, as the panes' text reads.
    pub side: UiFont,
    pub side_small: UiFont,
    /// The terminal's font for jot's own use, where a command's output
    /// is shown outside a terminal.
    pub mono: UiFont,
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
    /// Its name for swash's caches, the same for every glyph: without one
    /// swash takes each glyph for a new font, and sets the font up again
    /// (its tables read, its hinting program run) for every glyph drawn.
    key: u64,
}

impl Face {
    fn new(bytes: Vec<u8>, index: u32) -> Option<Face> {
        let font = Font::from_bytes(bytes.as_slice(), FontSettings { collection_index: index, ..FontSettings::default() }).ok()?;
        static KEYS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let key = KEYS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Some(Face { font, bytes, index, key })
    }

    /// A glyph by number, hinted, as coverage: where its box starts right
    /// of the pen and above the baseline. With `lcd`, three coverages a
    /// pixel, one for each of its stripes, red first (or blue, for BGR).
    fn raster(&self, ctx: &mut ScaleContext, id: u16, px: f32, lcd: Option<Lcd>) -> Raster {
        let wide = if lcd.is_some() { 3.0 } else { 1.0 };
        let image = swash::FontRef::from_index(&self.bytes, self.index as usize).and_then(|font| {
            let mut scaler = ctx.builder_with_id(font, [self.key, self.index as u64]).size(px).hint(true).build();
            Render::new(&[Source::Outline]).format(Format::Alpha).transform(Some(swash::zeno::Transform::scale(wide, 1.0))).render(&mut scaler, id)
        });
        let Some(img) = image else {
            return Raster { left: 0, top: 0, width: 0, height: 0, coverage: Vec::new(), lcd: false };
        };
        let (left, top, width, height) = (img.placement.left, img.placement.top, img.placement.width as usize, img.placement.height as usize);
        match lcd {
            None => Raster { left, top, width, height, coverage: img.data, lcd: false },
            Some(order) => {
                let (px0, w, coverage) = lcd_filter(&img.data, left, width, height, order);
                Raster { left: px0, top, width: w, height, coverage, lcd: true }
            }
        }
    }
}

/// The order of a screen's stripes, for subpixel text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lcd {
    Rgb,
    Bgr,
}

/// Coverage at three times the width made into a pixel's three stripes,
/// smoothed across its neighbours by FreeType's default LCD filter so
/// that edges are not fringed with color. Returns the first pixel, the
/// width in pixels, and the coverages, three a pixel.
fn lcd_filter(data: &[u8], left3: i32, w3: usize, h: usize, order: Lcd) -> (i32, usize, Vec<u8>) {
    const TAPS: [u32; 5] = [0x08, 0x4D, 0x56, 0x4D, 0x08];
    let first = (left3 - 2).div_euclid(3);
    let last = (left3 + w3 as i32 + 2 + 2).div_euclid(3);
    let w = (last - first).max(0) as usize;
    let mut out = vec![0u8; w * 3 * h];
    for y in 0..h {
        let row = &data[y * w3..(y + 1) * w3];
        let at = |sub: i32| -> u32 {
            let i = sub - left3;
            if i < 0 || i >= w3 as i32 { 0 } else { row[i as usize] as u32 }
        };
        for p in 0..w {
            for s in 0..3 {
                let sub = (first + p as i32) * 3 + s as i32;
                let v: u32 = (0..5).map(|k| TAPS[k] * at(sub - 2 + k as i32)).sum::<u32>() / 256;
                let ch = if order == Lcd::Bgr { 2 - s } else { s };
                out[(y * w + p) * 3 + ch] = v.min(255) as u8;
            }
        }
    }
    (first, w, out)
}

/// A glyph's coverage as rasterized, before it is placed in a cell.
struct Raster {
    left: i32,
    /// From the baseline up to the top of its box.
    top: i32,
    width: usize,
    height: usize,
    coverage: Vec<u8>,
    /// Three coverages a pixel, red green blue, rather than one.
    lcd: bool,
}

thread_local! {
    /// Faces as loaded, by what they were asked for: a font is read and
    /// parsed once, whatever sizes it is made in afterwards (a change of
    /// size, or of the screen's scale, makes only the sizes again).
    static FACES: std::cell::RefCell<HashMap<String, (Rc<Face>, Rc<Face>)>> = Default::default();
    /// The terminal's fallbacks: for each character fontconfig was asked
    /// about, the file with it; and those files' faces.
    static TERM_BY_CHAR: std::cell::RefCell<HashMap<char, Option<String>>> = Default::default();
    static TERM_BY_FILE: std::cell::RefCell<HashMap<String, Rc<Face>>> = Default::default();
}

/// A face loaded once, under `key`.
fn loaded(key: String, load: impl FnOnce() -> (Rc<Face>, Rc<Face>)) -> (Rc<Face>, Rc<Face>) {
    if let Some(f) = FACES.with(|m| m.borrow().get(&key).cloned()) {
        return f;
    }
    let f = load();
    FACES.with(|m| m.borrow_mut().insert(key, f.clone()));
    f
}

/// The terminal's font, regular and bold.
fn terminal_font(family: &str, with_bold: bool) -> (Rc<Face>, Rc<Face>) {
    loaded(format!("terminal|{family}|{with_bold}"), || {
        if !with_bold {
            let (regular, _) = terminal_font(family, true);
            return (regular.clone(), regular);
        }
        if !family.eq_ignore_ascii_case(TERMINAL_DEFAULT) {
            if let Some(regular) = find(family, "medium").or_else(|| find(family, "regular")) {
                let bold = find(family, "bold").unwrap_or_else(|| find(family, "regular").unwrap_or_else(|| Face::new(FIRA.to_vec(), 0).unwrap()));
                return (Rc::new(regular), Rc::new(bold));
            }
        }
        (Rc::new(Face::new(FIRA.to_vec(), 0).unwrap()), Rc::new(Face::new(FIRA_BOLD.to_vec(), 0).unwrap()))
    })
}

fn interface_font(family: &str) -> Rc<Face> {
    loaded(format!("interface|{family}"), || {
        if !family.eq_ignore_ascii_case(INTERFACE_DEFAULT) {
            if let Some(f) = find(family, "medium").or_else(|| find(family, "regular")) {
                let f = Rc::new(f);
                return (f.clone(), f);
            }
        }
        let f = Rc::new(Face::new(GEIST.to_vec(), 0).unwrap());
        (f.clone(), f)
    })
    .0
}

/// The font families installed, by name, each once, A to Z: only those
/// whose letters are all one width with `mono`. The fonts jot has
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
        let mono = UiFont::new(&regular, choice.size * scale, choice.lcd);
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
            lcd: choice.lcd,
            heading: UiFont::new(&ui, ui_px * 1.3, choice.lcd),
            small: UiFont::new(&ui, ui_px * 0.88, choice.lcd),
            side: UiFont::new(&ui, ui_px * 1.08, choice.lcd),
            side_small: UiFont::new(&ui, ui_px * 0.96, choice.lcd),
            ui: UiFont::new(&ui, ui_px, choice.lcd),
            mono,
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

    /// Asks fontconfig for a font with `c`, once per character for as
    /// long as jot runs (not again with each size).
    fn find_fallback(&mut self, c: char) -> Option<usize> {
        let file = match TERM_BY_CHAR.with(|m| m.borrow().get(&c).cloned()) {
            Some(file) => file?,
            None => {
                let pattern = format!("monospace:charset={:x}:color=false", c as u32);
                let file = Command::new("fc-match").args(["-f", "%{file}", &pattern]).output().ok().and_then(|o| String::from_utf8(o.stdout).ok());
                TERM_BY_CHAR.with(|m| m.borrow_mut().insert(c, file.clone()));
                file?
            }
        };
        if let Some(&i) = self.fallback_files.get(&file) {
            return (self.fallback[i].font.lookup_glyph_index(c) != 0).then_some(i);
        }
        let font = match TERM_BY_FILE.with(|m| m.borrow().get(&file).cloned()) {
            Some(f) => f,
            None => {
                let f = Rc::new(Face::new(std::fs::read(&file).ok()?, 0)?);
                TERM_BY_FILE.with(|m| m.borrow_mut().insert(file.clone(), f.clone()));
                f
            }
        };
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
        Some(Glyph { left: (cells * self.cell_w - w) / 2, top: (self.cell_h - h) / 2, width: w as usize, height: h as usize, coverage: Vec::new(), color: Some(pixels), lcd: false })
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
                    let r = face.raster(&mut self.scaler, face.font.lookup_glyph_index(c), self.px, self.lcd);
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
            let r = face.raster(&mut self.scaler, id, self.px, self.lcd);
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
            return Glyph { left: r.left, top, width: r.width, height: r.height, coverage: r.coverage, color: None, lcd: r.lcd };
        }
        let n = if r.lcd { 3 } else { 1 };
        let slant = 0.2;
        let extra = (r.height as f32 * slant).ceil() as usize;
        let w = r.width + extra;
        let mut out = vec![0u8; w * r.height * n];
        // The shift is measured from the baseline, so the glyph leans
        // about where it stands.
        let base = r.top as f32;
        for y in 0..r.height {
            let shift = ((base - y as f32) * slant).max(0.0);
            let (whole, frac) = (shift.floor() as usize, shift.fract());
            for x in 0..r.width {
                for ch in 0..n {
                    let v = r.coverage[(y * r.width + x) * n + ch] as f32;
                    let at = x + whole;
                    if at < w {
                        let o = &mut out[(y * w + at) * n + ch];
                        *o = (*o as f32 + v * (1.0 - frac)).min(255.0) as u8;
                    }
                    if at + 1 < w {
                        let o = &mut out[(y * w + at + 1) * n + ch];
                        *o = (*o as f32 + v * frac).min(255.0) as u8;
                    }
                }
            }
        }
        Glyph { left: r.left, top, width: w, height: r.height, coverage: out, color: None, lcd: r.lcd }
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

/// An interface font at one size, for jot's own words.
pub struct UiFont {
    face: Rc<Face>,
    scaler: ScaleContext,
    pub lcd: Option<Lcd>,
    pub px: f32,
    pub ascent: i32,
    pub height: i32,
    cache: HashMap<char, (fontdue::Metrics, Vec<u8>)>,
}

impl UiFont {
    fn new(face: &Rc<Face>, px: f32, lcd: Option<Lcd>) -> UiFont {
        // The face shared, parsed once: only the size is its own.
        let face = face.clone();
        let m = face.font.horizontal_line_metrics(px).expect("line metrics");
        // The ascent on a whole pixel, as hinting puts the tops of letters.
        UiFont { face, scaler: ScaleContext::new(), lcd, px, ascent: m.ascent.round() as i32, height: (m.ascent - m.descent).ceil() as i32, cache: HashMap::new() }
    }

    /// A character's coverage, hinted as the terminal's are, with the
    /// font's own advance so words keep their width. A character the
    /// interface's font lacks (a program's title often has symbols: a
    /// spinner, a star, an emoji) is drawn from a font that has it, as
    /// fontconfig finds; none has it, it is left out, not drawn as the
    /// font's box for a missing letter.
    pub fn glyph(&mut self, c: char) -> &(fontdue::Metrics, Vec<u8>) {
        if !self.cache.contains_key(&c) {
            let own = self.face.font.lookup_glyph_index(c) != 0 || c.is_whitespace();
            let fallback = if own { None } else { ui_fallback(c) };
            let face = fallback.as_deref().unwrap_or(&self.face);
            let made = if own || fallback.is_some() {
                let mut m = face.font.metrics(c, self.px);
                let r = face.raster(&mut self.scaler, face.font.lookup_glyph_index(c), self.px, self.lcd);
                m.xmin = r.left;
                m.width = r.width;
                m.height = r.height;
                m.ymin = r.top - r.height as i32;
                (m, r.coverage)
            } else {
                (fontdue::Metrics::default(), Vec::new())
            };
            self.cache.insert(c, made);
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
            return Some(Glyph { left: 0, top: 0, width: wu, height: hu, coverage: b.cov, color: None, lcd: false });
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
            return Some(Glyph { left: 0, top: 0, width: wu, height: hu, coverage: b.cov, color: None, lcd: false });
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
        Some(Glyph { left: 0, top: 0, width: wu, height: hu, coverage: b.cov, color: None, lcd: false })
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

/// Subpixel text as the setting asks: "rgb", "bgr" or "none", or "auto"
/// to do as the desktop does. Chrome and other X11 programs follow
/// Xft.rgba; on Wayland, GNOME's own setting says.
pub fn lcd_for(setting: &str) -> Option<Lcd> {
    match setting.trim().to_lowercase().as_str() {
        "rgb" => return Some(Lcd::Rgb),
        "bgr" => return Some(Lcd::Bgr),
        "none" | "gray" | "grayscale" | "off" => return None,
        _ => {}
    }
    static FOUND: std::sync::OnceLock<Option<Lcd>> = std::sync::OnceLock::new();
    *FOUND.get_or_init(|| {
        let run = |cmd: &str, args: &[&str]| Command::new(cmd).args(args).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
        let from = |order: &str| match order.trim().trim_matches('\'') {
            "rgb" => Some(Lcd::Rgb),
            "bgr" => Some(Lcd::Bgr),
            _ => None,
        };
        if std::env::var_os("DISPLAY").is_some() {
            if let Some(out) = run("xrdb", &["-query"]) {
                if let Some(line) = out.lines().find(|l| l.starts_with("Xft.rgba:")) {
                    return from(line.trim_start_matches("Xft.rgba:"));
                }
            }
        }
        let aa = run("gsettings", &["get", "org.gnome.desktop.interface", "font-antialiasing"])?;
        if aa.trim().trim_matches('\'') != "rgba" {
            return None;
        }
        from(&run("gsettings", &["get", "org.gnome.desktop.interface", "font-rgba-order"])?)
    })
}

/// A font with `c` in it for the interface's text, asked of fontconfig
/// once per character and kept once loaded, shared by every interface
/// size.
fn ui_fallback(c: char) -> Option<std::rc::Rc<Face>> {
    thread_local! {
        static BY_CHAR: std::cell::RefCell<HashMap<char, Option<std::rc::Rc<Face>>>> = Default::default();
        static BY_FILE: std::cell::RefCell<HashMap<String, std::rc::Rc<Face>>> = Default::default();
    }
    if let Some(f) = BY_CHAR.with(|m| m.borrow().get(&c).cloned()) {
        return f;
    }
    let found = (|| {
        let pattern = format!("sans:charset={:x}:color=false", c as u32);
        let out = Command::new("fc-match").args(["-f", "%{file}", &pattern]).output().ok()?;
        let file = String::from_utf8(out.stdout).ok()?;
        let face = match BY_FILE.with(|m| m.borrow().get(&file).cloned()) {
            Some(f) => f,
            None => {
                let f = std::rc::Rc::new(Face::new(std::fs::read(&file).ok()?, 0)?);
                BY_FILE.with(|m| m.borrow_mut().insert(file, f.clone()));
                f
            }
        };
        (face.font.lookup_glyph_index(c) != 0).then_some(face)
    })();
    BY_CHAR.with(|m| m.borrow_mut().insert(c, found.clone()));
    found
}
