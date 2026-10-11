//! Software rasterizer: draws the display list into a 0RGB pixel buffer.

use std::collections::HashMap;
use std::rc::Rc;

use crate::css::Color;
use crate::font::{FontKey, Fonts};
use crate::layout::{Item, Rect};

/// A decoded image, non-premultiplied 0xAARRGGBB.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

pub struct Canvas<'a> {
    pub pixels: &'a mut [u32],
    pub width: usize,
    clip: Vec<(i32, i32, i32, i32)>,
}

impl<'a> Canvas<'a> {
    pub fn new(pixels: &'a mut [u32], width: usize, height: usize) -> Self {
        Canvas { pixels, width, clip: vec![(0, 0, width as i32, height as i32)] }
    }

    /// The current clip as (x0, y0, x1, y1), exclusive.
    fn bounds(&self) -> (i32, i32, i32, i32) {
        *self.clip.last().expect("clip")
    }

    pub fn push_clip(&mut self, r: Rect) {
        let (x0, y0, x1, y1) = self.bounds();
        let nx0 = (r.x.round() as i32).max(x0);
        let ny0 = (r.y.round() as i32).max(y0);
        let nx1 = ((r.x + r.w).round() as i32).min(x1);
        let ny1 = ((r.y + r.h).round() as i32).min(y1);
        self.clip.push((nx0, ny0, nx1.max(nx0), ny1.max(ny0)));
    }

    pub fn pop_clip(&mut self) {
        if self.clip.len() > 1 {
            self.clip.pop();
        }
    }

    #[inline]
    fn blend(&mut self, x: i32, y: i32, c: Color, coverage: u32) {
        let alpha = c.a as u32 * coverage / 255;
        if alpha == 0 {
            return;
        }
        let i = y as usize * self.width + x as usize;
        if alpha >= 255 {
            self.pixels[i] = (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32;
            return;
        }
        let dst = self.pixels[i];
        let inv = 255 - alpha;
        let mix = |src: u8, shift: u32| ((src as u32 * alpha + ((dst >> shift) & 0xff) * inv) / 255) << shift;
        self.pixels[i] = mix(c.r, 16) | mix(c.g, 8) | mix(c.b, 0);
    }

    pub fn fill_rect(&mut self, r: Rect, c: Color) {
        if c.a == 0 {
            return;
        }
        let (cx0, cy0, cx1, cy1) = self.bounds();
        let x0 = (r.x.round() as i32).max(cx0);
        let y0 = (r.y.round() as i32).max(cy0);
        let x1 = ((r.x + r.w).round() as i32).min(cx1);
        let y1 = ((r.y + r.h).round() as i32).min(cy1);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        if c.a == 255 {
            let px = (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32;
            for y in y0..y1 {
                let row = y as usize * self.width;
                self.pixels[row + x0 as usize..row + x1 as usize].fill(px);
            }
        } else {
            for y in y0..y1 {
                for x in x0..x1 {
                    self.blend(x, y, c, 255);
                }
            }
        }
    }

    /// Fills a rounded rectangle, optionally minus an inner rounded
    /// rectangle (for borders), with anti-aliased corners.
    pub fn fill_rounded(&mut self, outer: Rect, radius: f32, c: Color, hole: Option<(Rect, f32)>) {
        if c.a == 0 || outer.w <= 0.0 || outer.h <= 0.0 {
            return;
        }
        let radius = radius.min(outer.w / 2.0).min(outer.h / 2.0).max(0.0);
        if radius < 0.5 && hole.is_none() {
            return self.fill_rect(outer, c);
        }
        let (cx0, cy0, cx1, cy1) = self.bounds();
        let x0 = (outer.x.floor() as i32).max(cx0);
        let y0 = (outer.y.floor() as i32).max(cy0);
        let x1 = ((outer.x + outer.w).ceil() as i32).min(cx1);
        let y1 = ((outer.y + outer.h).ceil() as i32).min(cy1);
        for y in y0..y1 {
            for x in x0..x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let mut cov = rounded_coverage(outer, radius, px, py);
                if let Some((inner, ir)) = hole {
                    cov = (cov - rounded_coverage(inner, ir, px, py)).max(0.0);
                }
                if cov > 0.0 {
                    self.blend(x, y, c, (cov * 255.0) as u32);
                }
            }
        }
    }

    pub fn draw_text(&mut self, fonts: &Fonts, x: f32, baseline: f32, text: &str, key: FontKey, size: f32, c: Color) {
        let (cx0, cy0, cx1, cy1) = self.bounds();
        let mut pen = x;
        let by = baseline.round() as i32;
        for ch in text.chars() {
            let advance = fonts.advance(key, ch, size);
            if pen > cx1 as f32 {
                break;
            }
            if !ch.is_whitespace() && pen + advance >= cx0 as f32 {
                let g = fonts.glyph(key, ch, size);
                let m = &g.metrics;
                let gx = pen.round() as i32 + m.xmin;
                let gy = by - m.ymin - m.height as i32;
                for row in 0..m.height as i32 {
                    let y = gy + row;
                    if y < cy0 || y >= cy1 {
                        continue;
                    }
                    for col in 0..m.width as i32 {
                        let x = gx + col;
                        if x < cx0 || x >= cx1 {
                            continue;
                        }
                        let cov = g.coverage[(row * m.width as i32 + col) as usize];
                        if cov > 0 {
                            self.blend(x, y, c, cov as u32);
                        }
                    }
                }
            }
            pen += advance;
        }
    }

    pub fn draw_image(&mut self, img: &Image, r: Rect) {
        if img.width == 0 || img.height == 0 || r.w < 1.0 || r.h < 1.0 {
            return;
        }
        let (cx0, cy0, cx1, cy1) = self.bounds();
        let x0 = (r.x.round() as i32).max(cx0);
        let y0 = (r.y.round() as i32).max(cy0);
        let x1 = ((r.x + r.w).round() as i32).min(cx1);
        let y1 = ((r.y + r.h).round() as i32).min(cy1);
        let sx = img.width as f32 / r.w;
        let sy = img.height as f32 / r.h;
        for y in y0..y1 {
            let iy = (((y as f32 + 0.5 - r.y) * sy) as u32).min(img.height - 1);
            let row = (iy * img.width) as usize;
            for x in x0..x1 {
                let ix = (((x as f32 + 0.5 - r.x) * sx) as u32).min(img.width - 1);
                let p = img.pixels[row + ix as usize];
                let c = Color { r: (p >> 16) as u8, g: (p >> 8) as u8, b: p as u8, a: (p >> 24) as u8 };
                self.blend(x, y, c, 255);
            }
        }
    }

    pub fn draw_border(&mut self, r: Rect, widths: [f32; 4], colors: [Color; 4], radius: f32) {
        let uniform = colors.iter().all(|c| *c == colors[0]);
        if radius >= 0.5 && uniform {
            let inner = Rect {
                x: r.x + widths[3],
                y: r.y + widths[0],
                w: r.w - widths[1] - widths[3],
                h: r.h - widths[0] - widths[2],
            };
            let inner_radius = (radius.min(r.w / 2.0).min(r.h / 2.0) - widths[0].max(widths[3])).max(0.0);
            self.fill_rounded(r, radius, colors[0], Some((inner, inner_radius)));
            return;
        }
        let [t, rt, b, l] = widths;
        self.fill_rect(Rect { x: r.x, y: r.y, w: r.w, h: t }, colors[0]);
        self.fill_rect(Rect { x: r.x, y: r.y + r.h - b, w: r.w, h: b }, colors[2]);
        self.fill_rect(Rect { x: r.x, y: r.y + t, w: l, h: r.h - t - b }, colors[3]);
        self.fill_rect(Rect { x: r.x + r.w - rt, y: r.y + t, w: rt, h: r.h - t - b }, colors[1]);
    }
}

/// Approximate coverage of the pixel centred at (px, py) by a rounded rect.
fn rounded_coverage(r: Rect, radius: f32, px: f32, py: f32) -> f32 {
    if r.w <= 0.0 || r.h <= 0.0 {
        return 0.0;
    }
    let radius = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
    // Signed distance to the rounded rectangle, negative inside.
    let cx = r.x + r.w / 2.0;
    let cy = r.y + r.h / 2.0;
    let qx = (px - cx).abs() - (r.w / 2.0 - radius);
    let qy = (py - cy).abs() - (r.h / 2.0 - radius);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    let inside = qx.max(qy).min(0.0);
    let dist = outside + inside - radius;
    (0.5 - dist).clamp(0.0, 1.0)
}

/// Paints the items that intersect the visible part of the page, shifted by
/// the scroll offset and the page's position in the window.
pub fn paint_items(
    canvas: &mut Canvas,
    items: &[Item],
    fonts: &Fonts,
    images: &HashMap<String, Rc<Image>>,
    dx: f32,
    dy: f32,
) {
    let (_, clip_top, _, clip_bottom) = canvas.bounds();
    let view_top = clip_top as f32 - dy;
    let view_bottom = clip_bottom as f32 - dy;
    let visible = |top: f32, bottom: f32| bottom >= view_top && top <= view_bottom;
    let shift = |r: &Rect| Rect { x: r.x + dx, y: r.y + dy, ..*r };
    for item in items {
        match item {
            Item::Fill { rect, color, radius } => {
                if visible(rect.y, rect.y + rect.h) {
                    if *radius >= 0.5 {
                        canvas.fill_rounded(shift(rect), *radius, *color, None);
                    } else {
                        canvas.fill_rect(shift(rect), *color);
                    }
                }
            }
            Item::Border { rect, widths, colors, radius } => {
                if visible(rect.y, rect.y + rect.h) {
                    canvas.draw_border(shift(rect), *widths, *colors, *radius);
                }
            }
            Item::Text { x, y, text, font, size, color } => {
                if visible(y - size * 1.2, y + size * 0.5) {
                    canvas.draw_text(fonts, x + dx, y + dy, text, *font, *size, *color);
                }
            }
            Item::Image { rect, src } => {
                if visible(rect.y, rect.y + rect.h)
                    && let Some(img) = images.get(&**src) {
                        canvas.draw_image(img, shift(rect));
                    }
            }
            Item::PushClip(rect) => canvas.push_clip(shift(rect)),
            Item::PopClip => canvas.pop_clip(),
        }
    }
}
