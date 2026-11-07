//! Emoji in color, from Twemoji, as Discord and X draw them: Mozilla's
//! font of them has each emoji as outlines filled with flat colors, one
//! over another, drawn here at whatever size a cell is, sharp. As jog
//! draws its emoji.

/// Twemoji, as Discord and X have them, in Mozilla's font of them, whose
/// emoji are outlines filled with flat colors, one over another: drawn
/// at any size, sharp.
const TWEMOJI: &[u8] = include_bytes!("../assets/Twemoji.Mozilla.ttf");

/// The layers of an emoji as the font has them: which outline, in which
/// color, the first lowest.
#[derive(Default)]
struct Layers {
    outline: Option<ttf_parser::GlyphId>,
    layers: Vec<(ttf_parser::GlyphId, ttf_parser::RgbaColor)>,
}

impl<'a> ttf_parser::colr::Painter<'a> for Layers {
    fn outline_glyph(&mut self, glyph: ttf_parser::GlyphId) {
        self.outline = Some(glyph);
    }

    fn paint(&mut self, paint: ttf_parser::colr::Paint<'a>) {
        // The font is of flat colors only.
        if let (Some(glyph), ttf_parser::colr::Paint::Solid(color)) = (self.outline, paint) {
            self.layers.push((glyph, color));
        }
    }

    fn push_clip(&mut self) {}
    fn push_clip_box(&mut self, _: ttf_parser::colr::ClipBox) {}
    fn pop_clip(&mut self) {}
    fn push_layer(&mut self, _: ttf_parser::colr::CompositeMode) {}
    fn pop_layer(&mut self) {}
    fn push_transform(&mut self, _: ttf_parser::Transform) {}
    fn pop_transform(&mut self) {}
}

/// An outline of the font laid onto a rasterizer: its points taken from
/// the font's units, up being up, to the square's pixels, down being down.
struct Trace<'a> {
    fill: &'a mut ab_glyph_rasterizer::Rasterizer,
    at: (f32, f32),
    start: (f32, f32),
    fit: f32,
    left: f32,
    base: f32,
}

impl Trace<'_> {
    fn point(&self, x: f32, y: f32) -> ab_glyph_rasterizer::Point {
        ab_glyph_rasterizer::point(self.left + x * self.fit, self.base - y * self.fit)
    }
}

impl ttf_parser::OutlineBuilder for Trace<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        (self.at, self.start) = ((x, y), (x, y));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let (from, to) = (self.point(self.at.0, self.at.1), self.point(x, y));
        self.fill.draw_line(from, to);
        self.at = (x, y);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (from, via, to) = (self.point(self.at.0, self.at.1), self.point(x1, y1), self.point(x, y));
        self.fill.draw_quad(from, via, to);
        self.at = (x, y);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (from, a, b, to) = (self.point(self.at.0, self.at.1), self.point(x1, y1), self.point(x2, y2), self.point(x, y));
        self.fill.draw_cubic(from, a, b, to);
        self.at = (x, y);
    }

    fn close(&mut self) {
        let (x, y) = self.start;
        self.line_to(x, y);
    }
}

/// An emoji `side` pixels square from Twemoji, each layer's outline filled
/// at that size and laid over the ones before; its alpha in the top byte,
/// its colors as they are. `None` for one the font has no colors for.
pub fn twemoji(which: &str, side: u32) -> Option<(i32, i32, Vec<u32>)> {
    let face = ttf_parser::Face::parse(TWEMOJI, 0).ok()?;
    let glyph = face.glyph_index(which.chars().next()?)?;
    let mut painted = Layers::default();
    face.paint_color_glyph(glyph, 0, ttf_parser::RgbaColor::new(0, 0, 0, 255), &mut painted)?;
    if painted.layers.is_empty() {
        return None;
    }
    // All the layers together, in the font's units: the square is fitted
    // to that, so that every emoji fills it the same.
    let mut around = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
    for (layer, _) in &painted.layers {
        if let Some(rect) = face.glyph_bounding_box(*layer) {
            around = [around[0].min(rect.x_min.into()), around[1].min(rect.y_min.into()), around[2].max(rect.x_max.into()), around[3].max(rect.y_max.into())];
        }
    }
    let (wide, high) = ((around[2] - around[0]).max(1) as f32, (around[3] - around[1]).max(1) as f32);
    let fit = side as f32 / wide.max(high);
    // Where the font's origin falls in the square, the emoji in the middle.
    let left = (side as f32 - wide * fit) / 2.0 - around[0] as f32 * fit;
    // The top of all its layers at the top of the emoji, down being up in
    // the font.
    let base = (side as f32 - high * fit) / 2.0 + around[3] as f32 * fit;
    let side = side as usize;
    // Colors kept weighed by how much of each pixel they cover, as layers
    // are laid one over another; weighed back out at the end.
    let mut sum = vec![[0f32; 4]; side * side];
    for (layer, color) in &painted.layers {
        let mut fill = ab_glyph_rasterizer::Rasterizer::new(side, side);
        face.outline_glyph(*layer, &mut Trace { fill: &mut fill, at: (0.0, 0.0), start: (0.0, 0.0), fit, left, base })?;
        let opacity = f32::from(color.alpha) / 255.0;
        fill.for_each_pixel(|at, cover| {
            let a = cover.min(1.0) * opacity;
            if a <= 0.0 {
                return;
            }
            let pixel = &mut sum[at];
            for (channel, value) in [color.red, color.green, color.blue].into_iter().enumerate() {
                pixel[channel] = pixel[channel] * (1.0 - a) + f32::from(value) * a;
            }
            pixel[3] = pixel[3] * (1.0 - a) + a;
        });
    }
    let side = side as i32;
    let pixels = sum
        .iter()
        .map(|[r, g, b, a]| {
            if *a <= 0.0 {
                return 0;
            }
            let channel = |v: f32| (v / a).round().clamp(0.0, 255.0) as u32;
            ((a * 255.0).round() as u32) << 24 | channel(*r) << 16 | channel(*g) << 8 | channel(*b)
        })
        .collect();
    Some((side, side, pixels))
}
